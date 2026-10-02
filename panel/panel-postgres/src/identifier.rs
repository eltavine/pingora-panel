use panel_errors::{PanelError, Result};
use std::fmt;

/// A PostgreSQL identifier restricted to a quote-free, portable subset:
/// lowercase ASCII letters, digits and underscores, at most 63 bytes
/// (`NAMEDATALEN - 1`), and never the reserved `pg_` prefix.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SqlIdentifier(String);

impl SqlIdentifier {
    pub const MAX_BYTES: usize = 63;

    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (1..=Self::MAX_BYTES).contains(&bytes.len())
            && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
            && !value.starts_with("pg_");
        if !valid {
            return Err(PanelError::invalid_argument(format!(
                "{value:?} is not a lowercase PostgreSQL identifier of at most {} bytes",
                Self::MAX_BYTES
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The identifier as a delimited identifier. Validation guarantees it
    /// contains no double quotes.
    pub(crate) fn quoted(&self) -> String {
        format!("\"{}\"", self.0)
    }
}

impl fmt::Display for SqlIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quote_free_and_unreserved() {
        assert_eq!(SqlIdentifier::new("config").unwrap().quoted(), "\"config\"");
        assert!(SqlIdentifier::new("_panel_2").is_ok());
        assert!(SqlIdentifier::new("x".repeat(63)).is_ok());
        for invalid in ["", "Config", "1config", "con-fig", "con\"fig", "pg_catalog"] {
            assert!(SqlIdentifier::new(invalid).is_err(), "{invalid}");
        }
        assert!(SqlIdentifier::new("x".repeat(64)).is_err());
    }
}
