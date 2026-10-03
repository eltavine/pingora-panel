use crate::RequestScope;
use async_trait::async_trait;
use panel_errors::Result;
use std::{net::SocketAddr, time::Duration};

/// A TLS endpoint to test and the name to ask it for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsProbeTarget {
    pub address: SocketAddr,
    /// Sent as the server name and as the HTTP host.
    pub server_name: String,
}

/// What a client sees when it connects to a TLS endpoint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct TlsProbeReport {
    /// The version negotiated when every supported one is offered, such as
    /// `TLSv1.3`.
    pub protocol: String,
    /// IANA name of the negotiated cipher suite.
    pub cipher_suite: String,
    pub alpn: Option<String>,
    pub handshake: Duration,
    /// The presented certificates in DER, leaf first.
    pub chain: Vec<Vec<u8>>,
    /// Each version and whether it is accepted when offered alone.
    pub versions: Vec<(String, bool)>,
    /// The status of `HEAD /` when the endpoint speaks HTTP/1.1.
    pub http_status: Option<u16>,
    /// The `Strict-Transport-Security` header of that response.
    pub strict_transport_security: Option<String>,
}

/// Connects to TLS endpoints the way clients do.
#[async_trait]
pub trait TlsProbe: Send + Sync {
    async fn probe(&self, scope: RequestScope, target: TlsProbeTarget) -> Result<TlsProbeReport>;
}
