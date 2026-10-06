//! The TLS handshake's scripts (ADR 0039), which run on the client's hello
//! before rustls answers it: `ssl_client_hello_by_lua` and
//! `ssl_certificate_by_lua` of the site the hello's server name selects,
//! which may end the handshake or choose the certificate it presents, and
//! `ssl_session_fetch_by_lua`, which may find the session it offers to
//! resume. `ssl_session_store_by_lua` runs as rustls keeps a new session.

use super::{lua_phases::Report, ListenerContext};
use crate::{
    adapter::ActiveSnapshot, certificates::ChosenCertificates, listeners::SESSION_CACHE, lua::Hook,
};
use async_trait::async_trait;
use bytes::Bytes;
use panel_ir::LuaFallback;
use panel_lua::{Connection, Exchange, Handshake, NoHost, Outcome, Request, Scripts};
use pingora_core::{
    listeners::PreTlsProcess,
    protocols::{l4::stream::Stream as L4Stream, GetSocketDigest},
    Error, ErrorType, Result,
};
use rustls::{
    server::{ServerSessionMemoryCache, StoresServerSessions},
    sign::CertifiedKey,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;

/// TLS records of handshake messages (RFC 8446 §5.1).
const HANDSHAKE_RECORD: u8 = 22;
/// The largest plaintext a record holds (RFC 8446 §5.1).
const MOST_RECORD: usize = 1 << 14;
/// The most a client's hello may take, its records' headers included.
const MOST_HELLO: usize = 1 << 16;
/// How long a client may take to send its hello.
const HELLO_TIME: Duration = Duration::from_secs(10);

/// Runs the handshake scripts of a listener's sites.
pub(crate) struct HandshakeScripts {
    pub(super) listener: Arc<ListenerContext>,
    pub(super) active: ActiveSnapshot,
    pub(super) chosen: Arc<ChosenCertificates>,
    pub(super) sessions: Arc<Sessions>,
}

/// The sessions a listener resumes: rustls' own cache, which
/// `ssl_session_fetch_by_lua` adds the sessions it finds to, and whose new
/// sessions go to `ssl_session_store_by_lua`.
pub(crate) struct Sessions {
    cache: Arc<ServerSessionMemoryCache>,
    listener: Arc<ListenerContext>,
    active: ActiveSnapshot,
}

impl Sessions {
    pub(super) fn new(listener: Arc<ListenerContext>, active: ActiveSnapshot) -> Self {
        Self {
            cache: ServerSessionMemoryCache::new(SESSION_CACHE),
            listener,
            active,
        }
    }

    /// The session the hello offers to resume that is not held here: its
    /// first ticket when it offers TLS 1.3, its session ID otherwise.
    fn missing(&self, handshake: &Handshake) -> Option<Bytes> {
        let candidate = if handshake.versions.contains(&0x0304) {
            handshake.tickets.first()
        } else {
            Some(&handshake.session_id).filter(|id| !id.is_empty())
        }?;
        self.cache
            .get(candidate)
            .is_none()
            .then(|| candidate.clone())
    }

    /// Runs `ssl_session_store_by_lua` on a session rustls keeps.
    fn stored(&self, id: &[u8], session: &[u8]) {
        let Some(snapshot) = self.active.load_full() else {
            return;
        };
        let Some(plan) = snapshot.lua.clone().filter(|plan| !plan.disabled) else {
            return;
        };
        let (Some(hook), Ok(runtime)) = (
            plan.session_store.clone(),
            tokio::runtime::Handle::try_current(),
        ) else {
            return;
        };
        let report = Report::program(Arc::clone(&self.listener), Arc::clone(&snapshot));
        let mut exchange = Exchange::new(
            Request::default(),
            Connection {
                tls: true,
                ..Connection::default()
            },
        );
        exchange.handshake.session = Some(Bytes::copy_from_slice(id));
        exchange.handshake.serialized = Some(Bytes::copy_from_slice(session));
        runtime.spawn(async move {
            let mut scripts = plan.runtime.scripts(exchange);
            run(&mut scripts, &hook, &report).await;
        });
    }
}

impl fmt::Debug for Sessions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Sessions")
            .field("listener", &self.listener.id)
            .finish_non_exhaustive()
    }
}

impl StoresServerSessions for Sessions {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        self.stored(&key, &value);
        self.cache.put(key, value)
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.cache.get(key)
    }

    fn take(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.cache.take(key)
    }

    fn can_cache(&self) -> bool {
        self.cache.can_cache()
    }
}

/// Runs `hook` and reports the run; whether the handshake goes on.
async fn run(scripts: &mut Scripts, hook: &Hook, report: &Report) -> bool {
    let started = Instant::now();
    let outcome = scripts.run(hook.handler, &mut NoHost).await;
    report.finished(hook, scripts, &outcome, started.elapsed());
    match outcome {
        Outcome::Continue => true,
        Outcome::Failed(_) => matches!(hook.fallback, LuaFallback::Continue),
        Outcome::Respond | Outcome::Abort => false,
    }
}

/// Reads the records that hold the client's hello into `read`, and the
/// hello out of them; `None` when what came is no hello.
async fn read_hello(stream: &mut L4Stream, read: &mut Vec<u8>) -> std::io::Result<Option<Vec<u8>>> {
    let mut message = Vec::new();
    loop {
        let mut header = [0u8; 5];
        stream.read_exact(&mut header).await?;
        read.extend_from_slice(&header);
        let length = usize::from(u16::from_be_bytes([header[3], header[4]]));
        if header[0] != HANDSHAKE_RECORD || length == 0 || length > MOST_RECORD {
            return Ok(None);
        }
        let start = read.len();
        read.resize(start + length, 0);
        stream.read_exact(&mut read[start..]).await?;
        message.extend_from_slice(&read[start..]);
        if message.len() >= 4 {
            if message[0] != 1 {
                return Ok(None);
            }
            let wanted = 4 + message[1..4]
                .iter()
                .fold(0, |length, byte| (length << 8) | usize::from(*byte));
            if wanted > MOST_HELLO {
                return Ok(None);
            }
            if message.len() >= wanted {
                message.truncate(wanted);
                return Ok(Some(message));
            }
        }
        if read.len() > MOST_HELLO {
            return Ok(None);
        }
    }
}

/// The certificate scripts chose: `Some(None)` when they cleared the TLS
/// profile's and set none.
fn certified(
    handshake: &Handshake,
) -> std::result::Result<Option<Option<Arc<CertifiedKey>>>, String> {
    match (&handshake.chain, &handshake.key) {
        (None, None) => Ok(handshake.cleared.then_some(None)),
        (Some(chain), Some(key)) => {
            let chain = chain
                .iter()
                .map(|certificate| CertificateDer::from(certificate.clone()))
                .collect();
            let key = PrivateKeyDer::try_from(key.clone())
                .map_err(|error| format!("the private key cannot be used: {error}"))?;
            let signing = rustls::crypto::ring::sign::any_supported_type(&key)
                .map_err(|error| format!("the private key cannot be used: {error}"))?;
            let mut key = CertifiedKey::new(chain, signing);
            key.keys_match().map_err(|error| {
                format!("the private key does not match the certificate: {error}")
            })?;
            key.ocsp = handshake.ocsp.as_ref().map(|response| response.to_vec());
            Ok(Some(Some(Arc::new(key))))
        }
        (Some(_), None) => Err("the scripts set a certificate without its private key".into()),
        (None, Some(_)) => Err("the scripts set a private key without its certificate".into()),
    }
}

fn ended() -> Box<Error> {
    Error::explain(
        ErrorType::HandshakeError,
        "a TLS handshake script ended the handshake",
    )
}

#[async_trait]
impl PreTlsProcess for HandshakeScripts {
    async fn process(&self, stream: &mut L4Stream) -> Result<()> {
        let Some(snapshot) = self.active.load_full() else {
            return Ok(());
        };
        if !snapshot.scripted_handshakes.contains(&self.listener.id) {
            return Ok(());
        }
        let Some(plan) = snapshot.lua.clone().filter(|plan| !plan.disabled) else {
            return Ok(());
        };
        let mut read = Vec::new();
        let hello = tokio::time::timeout(HELLO_TIME, read_hello(stream, &mut read)).await;
        stream.rewind(&read);
        let hello = match hello {
            Ok(Ok(Some(hello))) => hello,
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(error)) => {
                return Err(Error::because(
                    ErrorType::ReadError,
                    "the TLS client hello could not be read",
                    error,
                ))
            }
            Err(_) => {
                return Err(Error::explain(
                    ErrorType::ReadTimedout,
                    "the client sent no TLS hello in time",
                ))
            }
        };
        let Some(mut handshake) = Handshake::parse(&hello) else {
            return Ok(());
        };
        let routing = &snapshot.routing;
        let listener = self.listener.id.as_str();
        let site = handshake
            .server_name
            .as_deref()
            .and_then(|name| routing.lookup(name))
            .filter(|entry| routing.serves(entry.site, listener))
            .map(|entry| entry.site)
            .or_else(|| routing.default_site(listener));
        let hooks = site.map(|site| &routing.site(site).lua);
        let hello_hook = hooks.and_then(|hooks| hooks.ssl_client_hello.as_ref());
        let cert_hook = hooks.and_then(|hooks| hooks.ssl_cert.as_ref());
        let fetch = plan
            .session_fetch
            .as_ref()
            .zip(self.sessions.missing(&handshake));
        if hello_hook.is_none() && cert_hook.is_none() && fetch.is_none() {
            return Ok(());
        }
        let digest = stream.get_socket_digest();
        let address = |local: bool| {
            let digest = digest.as_ref()?;
            let address = if local {
                digest.local_addr()
            } else {
                digest.peer_addr()
            };
            address?.as_inet().copied()
        };
        let server_name = handshake.server_name.clone().unwrap_or_default();
        handshake.session = fetch.as_ref().map(|(_, id)| id.clone());
        let mut exchange = Exchange::new(
            Request::default(),
            Connection {
                client: address(false),
                server: address(true),
                tls: true,
                server_name,
                ..Connection::default()
            },
        );
        exchange.handshake = handshake;
        let mut scripts = plan.runtime.scripts(exchange);
        let site_report =
            site.map(|site| Report::site(Arc::clone(&self.listener), Arc::clone(&snapshot), site));
        if let (Some(hook), Some(report)) = (hello_hook, &site_report) {
            if !run(&mut scripts, hook, report).await {
                return Err(ended());
            }
        }
        if let Some((hook, id)) = fetch {
            let report = Report::program(Arc::clone(&self.listener), Arc::clone(&snapshot));
            if !run(&mut scripts, hook, &report).await {
                return Err(ended());
            }
            let found = scripts.exchange().handshake.serialized.take();
            if let Some(session) = found {
                // A resumed session presents no certificate.
                self.sessions.cache.put(id.to_vec(), session.to_vec());
                return Ok(());
            }
        }
        let (Some(hook), Some(report)) = (cert_hook, &site_report) else {
            return Ok(());
        };
        if !run(&mut scripts, hook, report).await {
            return Err(ended());
        }
        let handshake = std::mem::take(&mut scripts.exchange().handshake);
        match certified(&handshake) {
            Ok(Some(key)) => self.chosen.choose(key),
            Ok(None) => {
                // A response stapled to the TLS profile's certificate.
                let Some(response) = &handshake.ocsp else {
                    return Ok(());
                };
                let certificates = snapshot.certificates.load();
                let presented = certificates.presented(listener, handshake.server_name.as_deref());
                if let Some(certificate) = presented {
                    let mut key = CertifiedKey::clone(&certificate.key);
                    key.ocsp = Some(response.to_vec());
                    self.chosen.choose(Some(Arc::new(key)));
                }
            }
            Err(message) => {
                report.write(hook, "", "error", &message);
                return Err(ended());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_present_a_full_certificate_or_none() {
        let mut handshake = Handshake::default();
        assert!(matches!(certified(&handshake), Ok(None)));
        handshake.cleared = true;
        assert!(matches!(certified(&handshake), Ok(Some(None))));
        let certified_key =
            rcgen::generate_simple_self_signed(vec!["shop.example".into()]).unwrap();
        handshake.chain = Some(vec![certified_key.cert.der().to_vec()]);
        assert!(certified(&handshake)
            .unwrap_err()
            .contains("without its private key"));
        handshake.key = Some(certified_key.signing_key.serialize_der());
        assert!(matches!(certified(&handshake), Ok(Some(Some(_)))));
        let other = rcgen::generate_simple_self_signed(vec!["other.example".into()]).unwrap();
        handshake.key = Some(other.signing_key.serialize_der());
        assert!(certified(&handshake)
            .unwrap_err()
            .contains("does not match"));
    }
}
