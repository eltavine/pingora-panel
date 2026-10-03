use crate::details::CertificateDetails;
use chrono::{DateTime, Utc};
pub use panel_domain::CertificateId;
use serde::{Deserialize, Serialize};

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

impl Certificate {
    /// The entity tag of this version, for conditional changes (RFC 9110 §8.8.3).
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}
