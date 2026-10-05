//! What HTTP policies do besides setting and removing fields (ADR 0037):
//! field lines they add, the `Server` field, CORS and compression.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Required by snapshots whose sites or routes name an HTTP policy.
pub const HTTP_POLICIES_CAPABILITY: &str = "http.policies";

/// A field line, its value a template of request variables.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderField {
    pub name: String,
    pub value: String,
}

/// What becomes of the upstream's `Server` field (RFC 9110 §10.2.4).
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerHeader {
    #[default]
    Keep,
    Remove,
    Replace {
        value: String,
    },
}

impl ServerHeader {
    pub fn is_keep(&self) -> bool {
        *self == Self::Keep
    }
}

/// The Fetch Standard's CORS protocol for the origins listed.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsPolicy {
    /// Origins such as `https://shop.example`, `*`, or patterns of one
    /// label such as `https://*.shop.example`.
    pub allowed_origins: Vec<String>,
    /// Methods a preflight allows besides the CORS-safelisted ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_methods: Vec<String>,
    /// Request fields a preflight allows, by case-insensitive name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_headers: Vec<String>,
    /// Response fields scripts may read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed_headers: Vec<String>,
    /// Whether requests with credentials are allowed; the origin is then
    /// echoed, never `*`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_credentials: bool,
    /// How long a preflight's answer may be cached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u32>,
}

/// A content coding responses may be compressed with.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionAlgorithm {
    Gzip,
    Brotli,
    Zstd,
}

/// Which responses are compressed, with the codings the client accepts.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompressionPolicy {
    pub algorithms: BTreeSet<CompressionAlgorithm>,
    /// Media types such as `text/html`, or ranges such as `text/*`.
    pub types: Vec<String>,
    /// Responses of fewer bytes are sent as they are.
    #[serde(default)]
    pub min_bytes: u64,
}
