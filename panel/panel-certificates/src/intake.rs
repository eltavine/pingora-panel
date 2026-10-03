use crate::details::{CertificateDetails, KeyAlgorithm};
use crate::pem::{self, CERTIFICATE};
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use rustls::sign::CertifiedKey;
use rustls_pki_types::{
    pem::SectionKind, CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer,
    PrivateSec1KeyDer,
};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use x509_parser::{
    certificate::X509Certificate,
    extensions::GeneralName,
    oid_registry::{
        OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_NIST_EC_P384, OID_PKCS1_RSAENCRYPTION,
        OID_SIG_ED25519,
    },
    prelude::FromDer,
};
use zeroize::Zeroizing;

const MAX_CHAIN_BYTES: usize = 64 * 1024;
const MAX_KEY_BYTES: usize = 16 * 1024;
const MAX_CHAIN_LENGTH: usize = 8;
const MIN_RSA_BITS: u32 = 2048;

/// A certificate and key that passed every check, in canonical PEM.
pub struct Accepted {
    pub details: CertificateDetails,
    /// The chain, leaf first.
    pub chain: String,
    pub key: Zeroizing<String>,
}

impl std::fmt::Debug for Accepted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Accepted")
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}

/// Checks a PEM chain, leaf first, and the PEM private key for its leaf.
///
/// The key is loaded with the TLS provider the gateway uses and must match
/// the leaf's public key; expired certificates are refused.
pub fn accept(chain: &str, key: &str, now: DateTime<Utc>) -> Result<Accepted> {
    let certificates = parse_chain(chain)?;
    let details = inspect(&certificates)?;
    if details.not_after <= now {
        return Err(PanelError::validation_failed(format!(
            "the certificate expired on {}",
            details.not_after.to_rfc3339()
        )));
    }
    let (label, key) = parse_key(key)?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|error| {
        PanelError::validation_failed(format!("the private key cannot be used: {error}"))
    })?;
    CertifiedKey::new(certificates.clone(), signing_key)
        .keys_match()
        .map_err(|_| {
            PanelError::validation_failed("the private key does not belong to the certificate")
        })?;
    Ok(Accepted {
        details,
        chain: certificates
            .iter()
            .map(|certificate| pem::encode(CERTIFICATE, certificate))
            .collect(),
        key: Zeroizing::new(pem::encode(label, key.secret_der())),
    })
}

/// Describes a PEM chain, leaf first, without a key, for example to show a
/// certificate before its key is supplied.
pub fn describe(chain: &str) -> Result<CertificateDetails> {
    inspect(&parse_chain(chain)?)
}

/// Describes a chain of DER certificates, leaf first, such as a server
/// presented in a handshake.
pub fn describe_der(chain: &[Vec<u8>]) -> Result<CertificateDetails> {
    if chain.is_empty() {
        return Err(PanelError::validation_failed(
            "the certificate chain holds no certificate",
        ));
    }
    let certificates: Vec<CertificateDer<'_>> = chain
        .iter()
        .map(|der| CertificateDer::from(der.as_slice()))
        .collect();
    inspect(&certificates)
}

fn parse_chain(text: &str) -> Result<Vec<CertificateDer<'static>>> {
    if text.len() > MAX_CHAIN_BYTES {
        return Err(PanelError::validation_failed(format!(
            "the certificate chain is larger than {MAX_CHAIN_BYTES} bytes"
        )));
    }
    let sections = pem::sections(text, "certificate chain")?;
    if let Some((kind, _)) = sections
        .iter()
        .find(|(kind, _)| *kind != SectionKind::Certificate)
    {
        return Err(PanelError::validation_failed(format!(
            "the certificate chain holds a {} block; it may only hold certificates",
            pem::label(*kind)
        )));
    }
    match sections.len() {
        0 => Err(PanelError::validation_failed(
            "the certificate chain holds no PEM certificate",
        )),
        length if length > MAX_CHAIN_LENGTH => Err(PanelError::validation_failed(format!(
            "the certificate chain holds more than {MAX_CHAIN_LENGTH} certificates"
        ))),
        _ => Ok(sections
            .into_iter()
            .map(|(_, der)| CertificateDer::from(der))
            .collect()),
    }
}

fn parse_key(text: &str) -> Result<(&'static str, PrivateKeyDer<'static>)> {
    if text.len() > MAX_KEY_BYTES {
        return Err(PanelError::validation_failed(format!(
            "the private key is larger than {MAX_KEY_BYTES} bytes"
        )));
    }
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        return Err(PanelError::validation_failed(
            "the private key is encrypted; decrypt it first",
        ));
    }
    let mut sections = pem::sections(text, "private key")?.into_iter();
    let (Some((kind, der)), None) = (sections.next(), sections.next()) else {
        return Err(PanelError::validation_failed(
            "the private key must be exactly one PEM key",
        ));
    };
    match kind {
        SectionKind::PrivateKey => Ok((pem::label(kind), PrivatePkcs8KeyDer::from(der).into())),
        SectionKind::RsaPrivateKey => Ok((pem::label(kind), PrivatePkcs1KeyDer::from(der).into())),
        SectionKind::EcPrivateKey => Ok((pem::label(kind), PrivateSec1KeyDer::from(der).into())),
        other => Err(PanelError::validation_failed(format!(
            "a {} block is not a private key",
            pem::label(other)
        ))),
    }
}

/// Describes a chain, leaf first, after checking that each certificate was
/// issued by the next.
pub(crate) fn inspect(certificates: &[CertificateDer<'_>]) -> Result<CertificateDetails> {
    let parsed = certificates
        .iter()
        .enumerate()
        .map(|(index, der)| match X509Certificate::from_der(der) {
            Ok(([], certificate)) => Ok(certificate),
            _ => Err(PanelError::validation_failed(format!(
                "certificate {} of the chain is not a DER X.509 certificate",
                index + 1
            ))),
        })
        .collect::<Result<Vec<_>>>()?;
    for (index, pair) in parsed.windows(2).enumerate() {
        if pair[0].issuer().as_raw() != pair[1].subject().as_raw() {
            return Err(PanelError::validation_failed(format!(
                "certificate {} of the chain did not issue certificate {}; list the chain leaf first",
                index + 2,
                index + 1
            )));
        }
    }
    let leaf = &parsed[0];
    let names = names(leaf)?;
    if names.is_empty() {
        return Err(PanelError::validation_failed(
            "the certificate names no DNS name or IP address in its subject alternative names",
        ));
    }
    let (key_algorithm, key_bits) = key_kind(leaf)?;
    Ok(CertificateDetails {
        subject: leaf.subject().to_string(),
        issuer: leaf.issuer().to_string(),
        serial: serial(leaf.raw_serial()),
        names,
        not_before: time(leaf.validity().not_before.timestamp())?,
        not_after: time(leaf.validity().not_after.timestamp())?,
        fingerprint: hex::encode(Sha256::digest(certificates[0].as_ref())),
        public_key_fingerprint: hex::encode(Sha256::digest(leaf.public_key().raw)),
        key_algorithm,
        key_bits,
        chain_length: u32::try_from(certificates.len()).unwrap_or(u32::MAX),
        self_signed: leaf.subject().as_raw() == leaf.issuer().as_raw(),
    })
}

fn names(leaf: &X509Certificate<'_>) -> Result<Vec<String>> {
    let extension = leaf.subject_alternative_name().map_err(|_| {
        PanelError::validation_failed("the certificate's subject alternative names are malformed")
    })?;
    Ok(extension
        .map(|extension| {
            extension
                .value
                .general_names
                .iter()
                .filter_map(|name| match name {
                    GeneralName::DNSName(name) => Some(name.to_ascii_lowercase()),
                    GeneralName::IPAddress(bytes) => match bytes.len() {
                        4 => <[u8; 4]>::try_from(*bytes)
                            .ok()
                            .map(|octets| IpAddr::from(octets).to_string()),
                        16 => <[u8; 16]>::try_from(*bytes)
                            .ok()
                            .map(|octets| IpAddr::from(octets).to_string()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default())
}

fn key_kind(leaf: &X509Certificate<'_>) -> Result<(KeyAlgorithm, u32)> {
    let info = leaf.public_key();
    let algorithm = &info.algorithm.algorithm;
    let unsupported = || {
        PanelError::validation_failed(
            "the certificate's key must be RSA, ECDSA P-256 or P-384, or Ed25519",
        )
    };
    if *algorithm == OID_PKCS1_RSAENCRYPTION {
        let bits = info
            .parsed()
            .map(|key| u32::try_from(key.key_size()).unwrap_or(0))
            .map_err(|_| unsupported())?;
        if bits < MIN_RSA_BITS {
            return Err(PanelError::validation_failed(format!(
                "RSA keys need at least {MIN_RSA_BITS} bits; this one has {bits}"
            )));
        }
        Ok((KeyAlgorithm::Rsa, bits))
    } else if *algorithm == OID_KEY_TYPE_EC_PUBLIC_KEY {
        let curve = info
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok());
        match curve {
            Some(curve) if curve == OID_EC_P256 => Ok((KeyAlgorithm::EcdsaP256, 256)),
            Some(curve) if curve == OID_NIST_EC_P384 => Ok((KeyAlgorithm::EcdsaP384, 384)),
            _ => Err(unsupported()),
        }
    } else if *algorithm == OID_SIG_ED25519 {
        Ok((KeyAlgorithm::Ed25519, 256))
    } else {
        Err(unsupported())
    }
}

/// Hexadecimal without the leading zero bytes DER adds to positive integers.
fn serial(raw: &[u8]) -> String {
    let start = raw
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(raw.len().saturating_sub(1));
    hex::encode(&raw[start..])
}

fn time(timestamp: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| PanelError::validation_failed("the certificate's validity is out of range"))
}
