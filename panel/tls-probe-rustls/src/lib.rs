#![forbid(unsafe_code)]

//! `TlsProbe` with rustls: connects to a TLS endpoint as a browser would and
//! reports what it presents. The presented certificate is recorded rather
//! than trusted, so endpoints with self-signed or expired certificates can
//! be examined too; handshake signatures are still verified.

use async_trait::async_trait;
use panel_application::{RequestScope, TlsProbe, TlsProbeReport, TlsProbeTarget};
use panel_errors::{PanelError, Result};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    ClientConfig, DigitallySignedStruct, ProtocolVersion, SignatureScheme,
    SupportedProtocolVersion,
};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{client::TlsStream, TlsConnector};

const VERSIONS: [(&str, &SupportedProtocolVersion); 2] = [
    ("TLSv1.2", &rustls::version::TLS12),
    ("TLSv1.3", &rustls::version::TLS13),
];
/// What browsers offer, HTTP/2 first.
const BROWSER_ALPN: &[&[u8]] = &[b"h2", b"http/1.1"];
const HTTP1_ALPN: &[&[u8]] = &[b"http/1.1"];
const MAX_RESPONSE_HEAD: usize = 16 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Accepts every certificate so it can be reported, while still verifying
/// that the server holds the key it presents.
#[derive(Debug)]
struct RecordingVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Probes endpoints with the ring provider, giving up on each step after a
/// timeout.
#[derive(Clone, Debug)]
pub struct RustlsProbe {
    timeout: Duration,
}

impl Default for RustlsProbe {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

impl RustlsProbe {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn connector(
        &self,
        versions: &[&'static SupportedProtocolVersion],
        alpn: &[&[u8]],
    ) -> Result<TlsConnector> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(versions)
            .map_err(|error| PanelError::internal(format!("TLS client setup: {error}")))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RecordingVerifier(provider)))
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        config.resumption = rustls::client::Resumption::disabled();
        Ok(TlsConnector::from(Arc::new(config)))
    }

    async fn connect(
        &self,
        target: &TlsProbeTarget,
        versions: &[&'static SupportedProtocolVersion],
        alpn: &[&[u8]],
    ) -> Result<TlsStream<TcpStream>> {
        let name = ServerName::try_from(target.server_name.clone()).map_err(|_| {
            PanelError::invalid_argument(format!("{:?} is not a server name", target.server_name))
        })?;
        let late = || {
            PanelError::deadline_exceeded(format!(
                "{} did not answer within {} seconds",
                target.address,
                self.timeout.as_secs()
            ))
        };
        let tcp = timeout(self.timeout, TcpStream::connect(target.address))
            .await
            .map_err(|_| late())?
            .map_err(|error| {
                PanelError::unavailable(format!("cannot connect to {}: {error}", target.address))
            })?;
        timeout(
            self.timeout,
            self.connector(versions, alpn)?.connect(name, tcp),
        )
        .await
        .map_err(|_| late())?
        .map_err(|error| {
            PanelError::unavailable(format!(
                "the TLS handshake with {} failed: {error}",
                target.address
            ))
        })
    }
}

#[async_trait]
impl TlsProbe for RustlsProbe {
    async fn probe(&self, _scope: RequestScope, target: TlsProbeTarget) -> Result<TlsProbeReport> {
        let every: Vec<_> = VERSIONS.iter().map(|(_, version)| *version).collect();
        let started = Instant::now();
        let stream = self.connect(&target, &every, BROWSER_ALPN).await?;
        let mut report = TlsProbeReport::default();
        report.handshake = started.elapsed();
        {
            let (_, connection) = stream.get_ref();
            report.protocol = connection
                .protocol_version()
                .map(version_name)
                .unwrap_or_default();
            report.cipher_suite = connection
                .negotiated_cipher_suite()
                .map(|suite| format!("{:?}", suite.suite()))
                .unwrap_or_default();
            report.alpn = connection
                .alpn_protocol()
                .map(|protocol| String::from_utf8_lossy(protocol).into_owned());
            report.chain = connection
                .peer_certificates()
                .map(|chain| {
                    chain
                        .iter()
                        .map(|certificate| certificate.to_vec())
                        .collect()
                })
                .unwrap_or_default();
        }
        let http1 = if report.alpn.as_deref() == Some("h2") {
            self.connect(&target, &every, HTTP1_ALPN).await.ok()
        } else {
            Some(stream)
        };
        if let Some(mut stream) = http1 {
            if let Ok(Ok((status, policy))) =
                timeout(self.timeout, head(&mut stream, &target.server_name)).await
            {
                report.http_status = status;
                report.strict_transport_security = policy;
            }
        }
        for (name, version) in VERSIONS {
            let accepted = self
                .connect(&target, &[version], BROWSER_ALPN)
                .await
                .is_ok();
            report.versions.push((name.to_owned(), accepted));
        }
        Ok(report)
    }
}

fn version_name(version: ProtocolVersion) -> String {
    match version {
        ProtocolVersion::TLSv1_2 => "TLSv1.2".into(),
        ProtocolVersion::TLSv1_3 => "TLSv1.3".into(),
        other => format!("{other:?}"),
    }
}

/// The status and Strict-Transport-Security of `HEAD /` over HTTP/1.1.
async fn head(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    host: &str,
) -> std::io::Result<(Option<u16>, Option<String>)> {
    stream
        .write_all(
            format!("HEAD / HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n").as_bytes(),
        )
        .await?;
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).await?;
        received.extend_from_slice(&buffer[..read]);
        let complete = received.windows(4).any(|window| window == b"\r\n\r\n");
        if read == 0 || complete || received.len() > MAX_RESPONSE_HEAD {
            break;
        }
    }
    let text = String::from_utf8_lossy(&received);
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok());
    let policy = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| {
            name.trim()
                .eq_ignore_ascii_case("strict-transport-security")
        })
        .map(|(_, value)| value.trim().to_owned());
    Ok((status, policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_application::RequestId;
    use panel_errors::ErrorCode;
    use rustls::ServerConfig;
    use rustls_pki_types::PrivateKeyDer;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    fn scope() -> RequestScope {
        RequestScope::new(RequestId::new("probe-1").unwrap())
    }

    /// A TLS 1.2 endpoint with one cipher suite that answers with HSTS over
    /// HTTP/1.1 and offers `alpn`.
    async fn endpoint(alpn: &[&[u8]]) -> (SocketAddr, Vec<u8>) {
        let certified = rcgen::generate_simple_self_signed(vec!["example.com".to_owned()]).unwrap();
        let leaf = certified.cert.der().to_vec();
        let provider = rustls::crypto::ring::default_provider();
        let suites = provider
            .cipher_suites
            .iter()
            .filter(|suite| {
                suite.suite() == rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
            })
            .copied()
            .collect();
        let mut config = ServerConfig::builder_with_provider(Arc::new(CryptoProvider {
            cipher_suites: suites,
            ..provider
        }))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certified.cert.der().clone()],
            PrivateKeyDer::try_from(certified.signing_key.serialize_der()).unwrap(),
        )
        .unwrap();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(tcp).await else {
                        return;
                    };
                    if stream.get_ref().1.alpn_protocol() == Some(b"h2") {
                        return;
                    }
                    let mut buffer = [0_u8; 1024];
                    let _ = stream.read(&mut buffer).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 204 No Content\r\nStrict-Transport-Security: max-age=600\r\n\r\n",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (address, leaf)
    }

    #[tokio::test]
    async fn probes_report_what_clients_negotiate_and_see() {
        let (address, leaf) = endpoint(&[]).await;
        let report = RustlsProbe::default()
            .probe(
                scope(),
                TlsProbeTarget {
                    address,
                    server_name: "example.com".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(report.protocol, "TLSv1.2");
        assert_eq!(
            report.cipher_suite,
            "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256"
        );
        assert_eq!(report.chain, [leaf]);
        assert_eq!(
            report.versions,
            [("TLSv1.2".to_owned(), true), ("TLSv1.3".to_owned(), false)]
        );
        assert_eq!(report.http_status, Some(204));
        assert_eq!(
            report.strict_transport_security.as_deref(),
            Some("max-age=600")
        );
    }

    #[tokio::test]
    async fn policies_are_read_over_http1_when_http2_is_negotiated() {
        let (address, _) = endpoint(BROWSER_ALPN).await;
        let report = RustlsProbe::default()
            .probe(
                scope(),
                TlsProbeTarget {
                    address,
                    server_name: "example.com".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(report.alpn.as_deref(), Some("h2"));
        assert_eq!(report.http_status, Some(204));
        assert_eq!(
            report.strict_transport_security.as_deref(),
            Some("max-age=600")
        );
    }

    #[tokio::test]
    async fn closed_ports_and_bad_names_are_reported() {
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let probe = RustlsProbe::new(Duration::from_secs(2));
        let refused = probe
            .probe(
                scope(),
                TlsProbeTarget {
                    address: closed,
                    server_name: "example.com".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code.as_str(), ErrorCode::UNAVAILABLE);
        let named = probe
            .probe(
                scope(),
                TlsProbeTarget {
                    address: closed,
                    server_name: "not a name".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(named.code.as_str(), ErrorCode::INVALID_ARGUMENT);
    }
}
