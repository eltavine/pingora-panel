//! PEM as the gateway reads it (RFC 7468).

use base64::{engine::general_purpose::STANDARD, Engine};
use panel_errors::{PanelError, Result};
use rustls_pki_types::pem::{self, SectionKind};

pub(crate) const CERTIFICATE: &str = "CERTIFICATE";

/// The sections the gateway's PEM reader recognizes, in order; others are
/// skipped like it skips them.
pub(crate) fn sections(text: &str, what: &str) -> Result<Vec<(SectionKind, Vec<u8>)>> {
    let mut reader = text.as_bytes();
    let mut sections = Vec::new();
    while let Some(section) = pem::from_buf(&mut reader).map_err(|error| {
        PanelError::validation_failed(format!("the {what} is not valid PEM: {error}"))
    })? {
        sections.push(section);
    }
    Ok(sections)
}

pub(crate) fn label(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Certificate => CERTIFICATE,
        SectionKind::PublicKey => "PUBLIC KEY",
        SectionKind::RsaPrivateKey => "RSA PRIVATE KEY",
        SectionKind::PrivateKey => "PRIVATE KEY",
        SectionKind::EcPrivateKey => "EC PRIVATE KEY",
        SectionKind::Crl => "X509 CRL",
        SectionKind::Csr => "CERTIFICATE REQUEST",
        _ => "non-certificate",
    }
}

/// Textual encoding with 64-character lines.
pub(crate) fn encode(label: &str, der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut text = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        text.push_str(std::str::from_utf8(line).unwrap_or_default());
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}
