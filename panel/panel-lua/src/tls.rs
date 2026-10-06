//! The TLS terms of cosockets, as the `lua_ssl_*` directives set them: the
//! authorities a server's certificate is checked against and the
//! certificates they revoked, the certificate the client presents, how many
//! intermediate certificates a chain may have, and the protocol versions
//! and cipher suites offered. Without terms, `sslhandshake` checks
//! certificates against the system's trusted roots.

use rustls::{
    client::{
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        WebPkiServerVerifier,
    },
    crypto::CryptoProvider,
    pki_types::{
        pem::PemObject, CertificateDer, CertificateRevocationListDer, PrivateKeyDer, ServerName,
        UnixTime,
    },
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme, SupportedProtocolVersion,
};
use std::sync::{Arc, OnceLock};

/// What `sslhandshake` offers and checks.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct TlsTerms {
    /// PEM certificates of the authorities to trust instead of the
    /// system's roots (`lua_ssl_trusted_certificate`).
    pub roots: Option<Vec<u8>>,
    /// PEM revocation lists of those authorities (`lua_ssl_crl`).
    pub crls: Option<Vec<u8>>,
    /// The PEM certificate chain and private key the client presents
    /// (`lua_ssl_certificate`, `lua_ssl_certificate_key`).
    pub client: Option<(Vec<u8>, Vec<u8>)>,
    /// The most intermediate certificates a server may send
    /// (`lua_ssl_verify_depth`).
    pub verify_depth: Option<usize>,
    /// `TLSv1.2` and `TLSv1.3` (`lua_ssl_protocols`); both when empty.
    pub versions: Vec<String>,
    /// IANA names of the TLS 1.2 suites offered (`lua_ssl_ciphers`); all
    /// when empty. TLS 1.3's are always offered, as OpenSSL offers them.
    pub cipher_suites: Vec<String>,
}

/// Terms a program holds, for the cosockets of the handlers that run on
/// them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TlsId(pub(crate) u32);

/// Client settings that check certificates, and settings that do not, for
/// `ssl_verify = false`.
#[derive(Clone, Debug)]
pub(crate) struct TlsConfigs {
    verifying: Arc<ClientConfig>,
    trusting: Arc<ClientConfig>,
}

impl TlsConfigs {
    pub(crate) fn get(&self, verify: bool) -> Arc<ClientConfig> {
        Arc::clone(if verify {
            &self.verifying
        } else {
            &self.trusting
        })
    }
}

/// Every program's terms, as each VM finds them.
#[derive(Clone, Debug, Default)]
pub(crate) struct TlsTable(pub Arc<[TlsConfigs]>);

/// Accepts any certificate chain but still checks the handshake's
/// signatures, for `ssl_verify = false`.
#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Refuses chains with more intermediate certificates than
/// `lua_ssl_verify_depth` allows before checking them as `inner` does.
#[derive(Debug)]
struct Depth {
    inner: Arc<dyn ServerCertVerifier>,
    most: usize,
}

impl ServerCertVerifier for Depth {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let counted = intermediates
            .iter()
            .filter(|certificate| !self_issued(certificate))
            .count();
        if counted > self.most {
            return Err(rustls::Error::General(format!(
                "certificate chain too long: {counted} intermediate certificates where lua_ssl_verify_depth allows {}",
                self.most
            )));
        }
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// One DER value: its tag, its contents and what follows it.
pub(crate) fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let size = usize::from(first & 0x7f);
        if size == 0 || size > 4 || rest.len() < size {
            return None;
        }
        let length = rest[..size]
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        (length, &rest[size..])
    };
    (rest.len() >= length).then(|| (tag, &rest[..length], &rest[length..]))
}

/// Whether a certificate names itself as its issuer, as a root a server
/// sends along does; OpenSSL does not count it in the chain's depth.
fn self_issued(certificate: &[u8]) -> bool {
    let issuer_and_subject = || {
        let (_, certificate, _) = der(certificate)?;
        let (_, mut fields, _) = der(certificate)?;
        if fields.first() == Some(&0xa0) {
            fields = der(fields)?.2;
        }
        let (_, _, after_serial) = der(fields)?;
        let (_, _, after_algorithm) = der(after_serial)?;
        let (_, issuer, after_issuer) = der(after_algorithm)?;
        let (_, _, after_validity) = der(after_issuer)?;
        let (_, subject, _) = der(after_validity)?;
        Some(issuer == subject)
    };
    issuer_and_subject().unwrap_or(false)
}

fn certificates(pem: &[u8], what: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let certificates = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{what} is not PEM: {error}"))?;
    if certificates.is_empty() {
        return Err(format!("{what} holds no certificate"));
    }
    Ok(certificates)
}

impl TlsTerms {
    /// The client settings the terms make.
    pub(crate) fn build(&self) -> Result<TlsConfigs, String> {
        let base = rustls::crypto::ring::default_provider();
        for name in &self.cipher_suites {
            if !base
                .cipher_suites
                .iter()
                .any(|suite| format!("{:?}", suite.suite()) == *name)
            {
                return Err(format!("cipher suite {name} is not offered"));
            }
        }
        let versions = match self.versions.as_slice() {
            [] => vec![&rustls::version::TLS12, &rustls::version::TLS13],
            names => names
                .iter()
                .map(|name| match name.as_str() {
                    "TLSv1.2" => Ok(&rustls::version::TLS12),
                    "TLSv1.3" => Ok(&rustls::version::TLS13),
                    other => Err(format!("{other} is not offered")),
                })
                .collect::<Result<Vec<&'static SupportedProtocolVersion>, _>>()?,
        };
        let cipher_suites: Vec<_> = base
            .cipher_suites
            .iter()
            .filter(|suite| {
                suite.version() == &rustls::version::TLS13
                    || self.cipher_suites.is_empty()
                    || self.cipher_suites.contains(&format!("{:?}", suite.suite()))
            })
            .copied()
            .collect();
        let provider = Arc::new(CryptoProvider {
            cipher_suites,
            ..base
        });
        let checking: Arc<dyn ServerCertVerifier> = match &self.roots {
            None => {
                if self.crls.is_some() {
                    return Err(
                        "revocation lists need the authorities of lua_ssl_trusted_certificate"
                            .into(),
                    );
                }
                Arc::new(
                    rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
                        .map_err(|error| format!("the system's trusted roots: {error}"))?,
                )
            }
            Some(pem) => {
                let mut roots = RootCertStore::empty();
                for certificate in certificates(pem, "lua_ssl_trusted_certificate")? {
                    roots.add(certificate).map_err(|error| {
                        format!("lua_ssl_trusted_certificate holds a certificate that cannot be trusted: {error}")
                    })?;
                }
                let mut builder = WebPkiServerVerifier::builder_with_provider(
                    Arc::new(roots),
                    Arc::clone(&provider),
                );
                if let Some(crls) = &self.crls {
                    let crls = CertificateRevocationListDer::pem_slice_iter(crls)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| format!("lua_ssl_crl is not PEM: {error}"))?;
                    builder = builder.with_crls(crls);
                }
                builder
                    .build()
                    .map_err(|error| format!("lua_ssl_trusted_certificate: {error}"))?
            }
        };
        let checking: Arc<dyn ServerCertVerifier> = match self.verify_depth {
            Some(most) => Arc::new(Depth {
                inner: checking,
                most,
            }),
            None => checking,
        };
        let settings = |verifier: Arc<dyn ServerCertVerifier>| -> Result<ClientConfig, String> {
            let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
                .with_protocol_versions(&versions)
                .map_err(|error| format!("the TLS terms do not fit together: {error}"))?
                .dangerous()
                .with_custom_certificate_verifier(verifier);
            match &self.client {
                None => Ok(builder.with_no_client_auth()),
                Some((chain, key)) => {
                    let chain = certificates(chain, "lua_ssl_certificate")?;
                    let key = PrivateKeyDer::from_pem_slice(key).map_err(|error| {
                        format!("lua_ssl_certificate_key is not a PEM private key: {error}")
                    })?;
                    builder.with_client_auth_cert(chain, key).map_err(|error| {
                        format!("lua_ssl_certificate and its key do not fit together: {error}")
                    })
                }
            }
        };
        Ok(TlsConfigs {
            verifying: Arc::new(settings(checking)?),
            trusting: Arc::new(settings(Arc::new(Unverified(Arc::clone(&provider))))?),
        })
    }
}

/// The settings of cosockets no terms apply to: the system's trusted roots,
/// or no check.
pub(crate) fn system(verify: bool) -> Result<Arc<ClientConfig>, String> {
    static SYSTEM: OnceLock<Result<TlsConfigs, String>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| TlsTerms::default().build())
        .as_ref()
        .map(|configs| configs.get(verify))
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};

    #[test]
    fn roots_name_themselves_as_their_issuers() {
        let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
        authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        authority
            .distinguished_name
            .push(DnType::CommonName, "authority");
        let root = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
        let mut leaf = CertificateParams::new(vec!["leaf.test".into()]).unwrap();
        leaf.distinguished_name.push(DnType::CommonName, "leaf");
        let leaf = leaf
            .signed_by(&KeyPair::generate().unwrap(), &root)
            .unwrap();
        assert!(self_issued(root.der()));
        assert!(!self_issued(leaf.der()));
        assert!(!self_issued(b"not a certificate"));
    }
}
