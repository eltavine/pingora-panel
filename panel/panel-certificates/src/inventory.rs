use crate::details::CertificateDetails;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

const MAX_ID_LEN: usize = 64;

/// Names a certificate of the inventory, such as `example.com` or
/// `intranet-wildcard`: lowercase letters, digits, hyphens and dots that
/// separate non-empty labels.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = String, example = "example.com"))]
#[serde(transparent)]
pub struct CertificateId(String);

impl CertificateId {
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let labels_valid = value.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
        if value.is_empty() || value.len() > MAX_ID_LEN || !labels_valid {
            return Err(PanelError::invalid_argument(format!(
                "certificate id {value:?} must be 1 to {MAX_ID_LEN} lowercase letters, digits and hyphens, in labels separated by dots"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file of the chain in the gateway's secret directory.
    pub fn chain_file(&self) -> String {
        format!("cert-{}.pem", self.0)
    }

    /// The file of the private key in the gateway's secret directory.
    pub fn key_file(&self) -> String {
        format!("cert-{}.key", self.0)
    }
}

impl fmt::Display for CertificateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CertificateId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// How a certificate entered the inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CertificateSource {
    Uploaded,
    SelfSigned,
    Acme,
}

/// A certificate of the inventory; its private key never leaves the
/// service that holds it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Certificate {
    pub id: CertificateId,
    pub source: CertificateSource,
    #[serde(flatten)]
    pub details: CertificateDetails,
    /// The chain, leaf first, in PEM.
    pub chain: String,
    /// Increases whenever the material is replaced.
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_file_name_safe_labels() {
        for valid in ["example.com", "a", "intranet-wildcard", "0.example"] {
            let id = CertificateId::new(valid).unwrap();
            assert_eq!(id.chain_file(), format!("cert-{valid}.pem"));
            assert_eq!(id.key_file(), format!("cert-{valid}.key"));
        }
        for invalid in [
            "",
            "Example.com",
            "a..b",
            ".a",
            "a.",
            "-a",
            "a-",
            "a/b",
            "*.example.com",
            "a_b",
            &"a".repeat(65),
        ] {
            assert!(CertificateId::new(invalid).is_err(), "{invalid}");
        }
        assert!(serde_json::from_str::<CertificateId>("\"../etc\"").is_err());
    }
}
