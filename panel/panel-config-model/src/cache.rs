//! Cache policies of the editable configuration (ADR 0043), named by sites
//! and routes, and the size of the gateway's cache store.

use crate::model::RouteCondition;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_none(value: &u64) -> bool {
    *value == 0
}

/// What the responses of the sites and routes naming it are cached by, and
/// for how long.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    /// Lowercase letters, digits, dots, hyphens and underscores.
    pub id: String,
    /// A disabled policy caches nothing for those naming it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Seconds responses with 200, 203, 204, 300, 301 or 308 are fresh for
    /// when their origin does not say; 0 for none.
    #[serde(default, skip_serializing_if = "is_none")]
    pub ttl_seconds: u64,
    /// Seconds by status, ahead of `ttl_seconds`; 0 keeps a status out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub status_ttls: BTreeMap<u16, u64>,
    /// A template of request variables; `$scheme$host$request_uri` when
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Request fields, lowercase, whose values keep responses apart besides
    /// those the response's `Vary` names.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub vary_headers: BTreeSet<String>,
    /// Whether the origin's `Cache-Control` and `Expires` decide what is
    /// stored and for how long.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub honor_origin: bool,
    /// Requests meeting any of these neither use nor fill the cache.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass: Vec<RouteCondition>,
    /// Seconds a stale response is served while it is revalidated, unless
    /// the origin says (RFC 5861).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stale_while_revalidate_seconds: u32,
    /// Seconds a stale response is served when the upstream fails, unless
    /// the origin says (RFC 5861).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stale_if_error_seconds: u32,
    /// The largest response stored; 8 MiB when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_object_bytes: Option<u64>,
    /// Whether responses carry `Cache-Status` (RFC 9211).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub status_header: bool,
}

impl CachePolicy {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            ttl_seconds: 0,
            status_ttls: BTreeMap::new(),
            key: None,
            vary_headers: BTreeSet::new(),
            honor_origin: true,
            bypass: Vec::new(),
            stale_while_revalidate_seconds: 0,
            stale_if_error_seconds: 0,
            max_object_bytes: None,
            status_header: true,
        }
    }

    /// The policy as gateways apply it.
    pub fn compile(&self) -> panel_ir::CachePolicy {
        let mut policy = panel_ir::CachePolicy::new(self.id.clone());
        policy.enabled = self.enabled;
        policy.ttl_seconds = self.ttl_seconds;
        policy.vary_headers = self
            .vary_headers
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect();
        policy.status_ttls = self.status_ttls.clone();
        policy.key.clone_from(&self.key);
        policy.honor_origin = self.honor_origin;
        policy.bypass = crate::compile::conditions(&self.bypass);
        policy.stale_while_revalidate_seconds = self.stale_while_revalidate_seconds;
        policy.stale_if_error_seconds = self.stale_if_error_seconds;
        policy.max_object_bytes = self.max_object_bytes;
        policy.status_header = self.status_header;
        policy
    }
}

/// The gateway's cache store.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CacheSettings {
    /// Bytes the store keeps; 256 MiB when none. A new size empties it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

impl CacheSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
