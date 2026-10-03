use ::pem::{EncodeConfig, LineEnding, Pem};

/// RFC 7468 textual encoding with LF line endings on every platform.
pub(crate) fn encode(label: &str, der: &[u8]) -> String {
    ::pem::encode_config(
        &Pem::new(label, der),
        EncodeConfig::new().set_line_ending(LineEnding::LF),
    )
}
