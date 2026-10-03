//! PEM as the gateway reads it (RFC 7468).

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

/// Textual encoding with LF line endings on every platform.
pub(crate) fn encode(label: &str, der: &[u8]) -> String {
    ::pem::encode_config(
        &::pem::Pem::new(label, der),
        ::pem::EncodeConfig::new().set_line_ending(::pem::LineEnding::LF),
    )
}
