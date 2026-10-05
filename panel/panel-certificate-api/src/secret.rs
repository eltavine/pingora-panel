use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

/// A secret such as a private key or a TSIG key: wiped from memory when
/// dropped and never printed.
#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<Zeroizing<String>> for Secret {
    fn from(value: Zeroizing<String>) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_travel_as_text_and_never_print() {
        let secret = Secret::new("sealed key material");
        assert_eq!(format!("{secret:?}"), "[redacted]");
        let encoded = serde_json::to_string(&secret).unwrap();
        assert_eq!(encoded, "\"sealed key material\"");
        assert_eq!(serde_json::from_str::<Secret>(&encoded).unwrap(), secret);
    }
}
