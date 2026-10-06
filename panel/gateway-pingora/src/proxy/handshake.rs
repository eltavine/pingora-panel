//! `ssl_client_hello_by_lua` and `ssl_certificate_by_lua` (ADR 0039): the
//! scripts of the site a TLS handshake's server name selects run on the
//! client's hello before rustls answers it, and may end the handshake or
//! choose the certificate it presents.

use super::{lua_phases::Report, ListenerContext};
use crate::{adapter::ActiveSnapshot, certificates::ChosenCertificates};
use async_trait::async_trait;
use panel_ir::LuaFallback;
use panel_lua::{Connection, Exchange, Handshake, NoHost, Outcome, Request};
use pingora_core::{
    listeners::PreTlsProcess,
    protocols::{l4::stream::Stream as L4Stream, GetSocketDigest},
    Error, ErrorType, Result,
};
use rustls::sign::CertifiedKey;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::{
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
            let key = CertifiedKey::new(chain, signing);
            key.keys_match().map_err(|error| {
                format!("the private key does not match the certificate: {error}")
            })?;
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
        let Some(handshake) = Handshake::parse(&hello) else {
            return Ok(());
        };
        let routing = &snapshot.routing;
        let listener = self.listener.id.as_str();
        let Some(site) = handshake
            .server_name
            .as_deref()
            .and_then(|name| routing.lookup(name))
            .filter(|entry| routing.serves(entry.site, listener))
            .map(|entry| entry.site)
            .or_else(|| routing.default_site(listener))
        else {
            return Ok(());
        };
        let hooks = &routing.site(site).lua;
        if hooks.ssl_client_hello.is_none() && hooks.ssl_cert.is_none() {
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
        let mut exchange = Exchange::new(
            Request::default(),
            Connection {
                client: address(false),
                server: address(true),
                tls: true,
                server_name: handshake.server_name.clone().unwrap_or_default(),
                ..Connection::default()
            },
        );
        exchange.handshake = handshake;
        let mut scripts = plan.runtime.scripts(exchange);
        let report = Report::site(Arc::clone(&self.listener), Arc::clone(&snapshot), site);
        let mut last = None;
        for hook in [&hooks.ssl_client_hello, &hooks.ssl_cert]
            .into_iter()
            .flatten()
        {
            let started = Instant::now();
            let outcome = scripts.run(hook.handler, &mut NoHost).await;
            report.finished(hook, &scripts, &outcome, started.elapsed());
            let go_on = match outcome {
                Outcome::Continue => true,
                Outcome::Failed(_) => matches!(hook.fallback, LuaFallback::Continue),
                Outcome::Respond | Outcome::Abort => false,
            };
            if !go_on {
                return Err(ended());
            }
            last = Some(hook);
        }
        let handshake = std::mem::take(&mut scripts.exchange().handshake);
        match certified(&handshake) {
            Ok(Some(key)) => self.chosen.choose(key),
            Ok(None) => {}
            Err(message) => {
                if let Some(hook) = last {
                    report.write(hook, "", "error", &message);
                }
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
