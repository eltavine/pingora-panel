//! Listening sockets and the fixed listener set of a data plane generation.

use crate::{
    certificates::TlsVersion,
    head_deadline::DEFAULT_HEAD_TIMEOUT,
    security::{ClientResolution, Networks},
};
use panel_errors::{PanelError, Result};
use panel_ir::{ListenerRef, TlsProfile};
use rustls::{
    crypto::CryptoProvider,
    server::{NoServerSessionStorage, ResolvesServerCert},
    ServerConfig, SupportedProtocolVersion,
};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{SocketAddr, TcpListener},
    sync::Arc,
    time::Duration,
};

const BACKLOG: i32 = 65_535;
/// Sessions each listener remembers for resumption.
pub(crate) const SESSION_CACHE: usize = 4_096;

/// Socket options fixed at bind time; changing any of them needs a new socket.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SocketKey {
    pub address: SocketAddr,
    pub reuse_port: bool,
    pub ipv6_only: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListenerPlan {
    pub id: String,
    pub socket: SocketKey,
    pub tls: bool,
    pub http1: bool,
    pub http2: bool,
    /// Protocols offered through ALPN: the enabled ones that the listener's
    /// own TLS profile allows.
    pub alpn_http1: bool,
    pub alpn_http2: bool,
    /// Handshake settings from the listener's own TLS profile.
    pub handshake: Handshake,
    /// How the client's address is learned from trusted proxies.
    pub client: ClientResolution,
    /// The longest a client may take to send a request head.
    pub head_timeout: Duration,
}

/// What a TLS listener accepts in every handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Handshake {
    pub oldest: TlsVersion,
    pub newest: TlsVersion,
    /// IANA names; empty accepts every supported suite.
    pub cipher_suites: Vec<String>,
    pub session_resumption: bool,
}

impl Default for Handshake {
    fn default() -> Self {
        Self {
            oldest: TlsVersion::Tls12,
            newest: TlsVersion::Tls13,
            cipher_suites: Vec::new(),
            session_resumption: true,
        }
    }
}

impl Handshake {
    fn from_profile(profile: &TlsProfile) -> Result<Self> {
        let version = |name: &str| {
            TlsVersion::parse(name).ok_or_else(|| {
                PanelError::validation_failed(format!(
                    "TLS profile {} names unsupported protocol {name}",
                    profile.id
                ))
            })
        };
        let handshake = Self {
            oldest: version(&profile.min_protocol)?,
            newest: profile
                .max_protocol
                .as_deref()
                .map(version)
                .transpose()?
                .unwrap_or(TlsVersion::Tls13),
            cipher_suites: profile.cipher_suites.clone(),
            session_resumption: profile.session_resumption,
        };
        if handshake.newest < handshake.oldest {
            return Err(PanelError::validation_failed(format!(
                "TLS profile {} accepts no protocol version",
                profile.id
            )));
        }
        Ok(handshake)
    }
}

impl ListenerPlan {
    pub(crate) fn from_ir(listener: &ListenerRef, profiles: &[TlsProfile]) -> Result<Self> {
        let address = listener.address.parse().map_err(|_| {
            PanelError::validation_failed(format!(
                "listener {} address {:?} is not an IP socket address",
                listener.id, listener.address
            ))
        })?;
        Ok(Self {
            id: listener.id.clone(),
            socket: SocketKey {
                address,
                reuse_port: listener.reuse_port,
                ipv6_only: listener.ipv6_only,
            },
            tls: listener.tls_profile_id.is_some(),
            http1: listener.protocols.http1,
            http2: listener.protocols.http2,
            alpn_http1: listener.protocols.http1 && offers(listener, profiles, "http/1.1"),
            alpn_http2: listener.protocols.http2 && offers(listener, profiles, "h2"),
            handshake: profile(listener, profiles)
                .map(Handshake::from_profile)
                .transpose()?
                .unwrap_or_default(),
            client: ClientResolution {
                trusted: Networks::parse(&listener.trusted_proxies).map_err(|error| {
                    PanelError::validation_failed(format!(
                        "listener {} trusts {}",
                        listener.id, error.message
                    ))
                })?,
                header: listener.real_ip_header,
            },
            head_timeout: match listener.request_head_timeout_ms {
                Some(0) => {
                    return Err(PanelError::validation_failed(format!(
                        "listener {} gives clients no time to send a request head",
                        listener.id
                    )))
                }
                Some(ms) => Duration::from_millis(ms),
                None => DEFAULT_HEAD_TIMEOUT,
            },
        })
    }

    /// The server side of the listener's handshakes, with certificates from
    /// `resolver`.
    #[cfg(test)]
    pub(crate) fn server_config(
        &self,
        resolver: Arc<dyn ResolvesServerCert>,
    ) -> Result<Arc<ServerConfig>> {
        self.server_config_with(
            resolver,
            rustls::server::ServerSessionMemoryCache::new(SESSION_CACHE),
            rustls::server::WebPkiClientVerifier::no_client_auth(),
        )
    }

    /// [`Self::server_config`], resuming sessions from `sessions` when the
    /// listener resumes them, and asking clients for certificates as
    /// `clients` says.
    pub(crate) fn server_config_with(
        &self,
        resolver: Arc<dyn ResolvesServerCert>,
        sessions: Arc<dyn rustls::server::StoresServerSessions>,
        clients: Arc<dyn rustls::server::danger::ClientCertVerifier>,
    ) -> Result<Arc<ServerConfig>> {
        let invalid = |detail: String| {
            PanelError::validation_failed(format!("listener {}: {detail}", self.id))
        };
        let base = rustls::crypto::ring::default_provider();
        let cipher_suites = if self.handshake.cipher_suites.is_empty() {
            base.cipher_suites.clone()
        } else {
            self.handshake
                .cipher_suites
                .iter()
                .map(|name| {
                    base.cipher_suites
                        .iter()
                        .find(|suite| format!("{:?}", suite.suite()) == *name)
                        .copied()
                        .ok_or_else(|| invalid(format!("unsupported cipher suite {name}")))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let versions: Vec<&'static SupportedProtocolVersion> = [
            (TlsVersion::Tls12, &rustls::version::TLS12),
            (TlsVersion::Tls13, &rustls::version::TLS13),
        ]
        .into_iter()
        .filter(|(version, _)| (self.handshake.oldest..=self.handshake.newest).contains(version))
        .map(|(_, supported)| supported)
        .collect();
        if let Some(version) = versions.iter().find(|version| {
            !cipher_suites
                .iter()
                .any(|suite| suite.version() == **version)
        }) {
            return Err(invalid(format!(
                "no cipher suite works with {:?}",
                version.version
            )));
        }
        let mut config = ServerConfig::builder_with_provider(Arc::new(CryptoProvider {
            cipher_suites,
            ..base
        }))
        .with_protocol_versions(&versions)
        .map_err(|error| invalid(format!("TLS settings do not fit together: {error}")))?
        .with_client_cert_verifier(clients)
        .with_cert_resolver(resolver);
        config.alpn_protocols = match (self.alpn_http1, self.alpn_http2) {
            (true, true) => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            (false, _) => vec![b"h2".to_vec()],
            (true, false) => vec![b"http/1.1".to_vec()],
        };
        if self.handshake.session_resumption {
            config.session_storage = sessions;
        } else {
            config.session_storage = Arc::new(NoServerSessionStorage {});
            config.send_tls13_tickets = 0;
        }
        Ok(Arc::new(config))
    }
}

fn profile<'a>(listener: &ListenerRef, profiles: &'a [TlsProfile]) -> Option<&'a TlsProfile> {
    listener
        .tls_profile_id
        .as_ref()
        .and_then(|id| profiles.iter().find(|profile| &profile.id == id))
}

fn offers(listener: &ListenerRef, profiles: &[TlsProfile], protocol: &str) -> bool {
    profile(listener, profiles)
        .is_none_or(|profile| profile.alpn.is_empty() || profile.alpn.contains(protocol))
}

pub(crate) fn bind(key: &SocketKey) -> io::Result<TcpListener> {
    let socket = Socket::new(
        Domain::for_address(key.address),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    #[cfg(unix)]
    {
        socket.set_reuse_address(true)?;
        if key.reuse_port {
            socket.set_reuse_port(true)?;
        }
    }
    if let (Some(only), true) = (key.ipv6_only, key.address.is_ipv6()) {
        socket.set_only_v6(only)?;
    }
    socket.bind(&key.address.into())?;
    socket.listen(BACKLOG)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_carry_socket_options_and_protocols() {
        let mut listener = ListenerRef::new("https", "[::]:0");
        listener.tls_profile_id = Some("tls".into());
        listener.reuse_port = true;
        listener.ipv6_only = Some(true);
        listener.protocols.http1 = false;
        let plan = ListenerPlan::from_ir(&listener, &[]).unwrap();
        assert!(plan.tls && !plan.http1 && plan.http2);
        assert!(!plan.alpn_http1 && plan.alpn_http2);
        assert!(plan.socket.reuse_port);
        assert_eq!(plan.socket.ipv6_only, Some(true));
        assert!(ListenerPlan::from_ir(&ListenerRef::new("bad", "localhost:80"), &[]).is_err());
    }

    #[test]
    fn the_listener_profile_narrows_the_alpn_offer() {
        let mut listener = ListenerRef::new("https", "127.0.0.1:443");
        listener.tls_profile_id = Some("tls".into());
        listener.protocols.http2 = true;
        let mut profile = TlsProfile {
            id: "tls".into(),
            certificate_secret_id: "cert.pem".into(),
            private_key_secret_id: "key.pem".into(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            alpn: ["http/1.1".to_owned()].into(),
        };
        let plan = ListenerPlan::from_ir(&listener, std::slice::from_ref(&profile)).unwrap();
        assert!(plan.http2 && plan.alpn_http1 && !plan.alpn_http2);

        profile.alpn.clear();
        let plan = ListenerPlan::from_ir(&listener, &[profile]).unwrap();
        assert!(plan.alpn_http1 && plan.alpn_http2);
    }

    #[test]
    fn handshakes_follow_the_listener_profile() {
        let mut listener = ListenerRef::new("https", "127.0.0.1:443");
        listener.tls_profile_id = Some("tls".into());
        let mut profile = TlsProfile {
            id: "tls".into(),
            certificate_secret_id: "cert.pem".into(),
            private_key_secret_id: "key.pem".into(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: Some("TLSv1.2".into()),
            cipher_suites: vec!["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".into()],
            session_resumption: false,
            alpn: ["http/1.1".to_owned()].into(),
        };
        let resolver = || Arc::new(rustls::server::ResolvesServerCertUsingSni::new());
        let plan = ListenerPlan::from_ir(&listener, std::slice::from_ref(&profile)).unwrap();
        assert_eq!(plan.handshake.newest, TlsVersion::Tls12);
        let config = plan.server_config(resolver()).unwrap();
        let suites: Vec<_> = config
            .crypto_provider()
            .cipher_suites
            .iter()
            .map(|suite| format!("{:?}", suite.suite()))
            .collect();
        assert_eq!(suites, ["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256"]);
        assert_eq!(config.alpn_protocols, [b"http/1.1".to_vec()]);
        assert_eq!(config.send_tls13_tickets, 0);

        profile.max_protocol = None;
        profile.session_resumption = true;
        profile.cipher_suites = panel_ir::tls::CIPHER_SUITES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        let plan = ListenerPlan::from_ir(&listener, std::slice::from_ref(&profile)).unwrap();
        let config = plan.server_config(resolver()).unwrap();
        assert_eq!(
            config.crypto_provider().cipher_suites.len(),
            panel_ir::tls::CIPHER_SUITES.len()
        );
        assert!(config.send_tls13_tickets > 0);

        profile.cipher_suites = vec!["TLS13_AES_128_GCM_SHA256".into()];
        let plan = ListenerPlan::from_ir(&listener, std::slice::from_ref(&profile)).unwrap();
        assert!(plan.server_config(resolver()).is_err());

        profile.cipher_suites.clear();
        profile.min_protocol = "TLSv1.3".into();
        profile.max_protocol = Some("TLSv1.2".into());
        assert!(ListenerPlan::from_ir(&listener, &[profile]).is_err());
    }

    #[test]
    fn occupied_addresses_fail_to_bind() {
        let key = SocketKey {
            address: "127.0.0.1:0".parse().unwrap(),
            reuse_port: false,
            ipv6_only: None,
        };
        let first = bind(&key).unwrap();
        let taken = SocketKey {
            address: first.local_addr().unwrap(),
            ..key
        };
        assert_eq!(bind(&taken).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }
}
