//! What requests must pass before a site or route handles them (ADR 0017).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Required by snapshots whose sites or routes name a security policy.
pub const REQUEST_SECURITY_CAPABILITY: &str = "request.security";

/// Required by snapshots whose listeners trust proxies to name the client.
pub const TRUSTED_PROXIES_CAPABILITY: &str = "listener.trusted-proxies";

/// Restrictions a request must pass, written once and named by sites and
/// routes; a request passes its site's policy and then its route's.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityPolicy {
    pub id: String,
    /// Client networks in CIDR notation; when any are listed, other
    /// clients are refused.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_cidrs: BTreeSet<String>,
    /// Client networks refused even when allowed.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub denied_cidrs: BTreeSet<String>,
    /// Requests per second per client address, the first form of a rate
    /// limit; kept so stored snapshots load unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_rate_per_second: Option<u64>,
    /// Methods allowed; empty allows every method.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_methods: BTreeSet<String>,
    /// Paths refused when they start with one of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_path_prefixes: Vec<String>,
    /// User agents refused when one of these case-insensitive regular
    /// expressions matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_user_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<RefererRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<BasicAuth>,
    /// The most bytes of request headers, names and values together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_header_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<u64>,
    /// The longest wait for the next part of a request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limits: Vec<RateLimit>,
    /// The most requests one client address may have in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u64>,
    /// The answer to requests over a limit, instead of 429.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_response: Option<LimitedResponse>,
}

/// Which pages may link to the site's resources (hotlink protection).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefererRule {
    /// Hosts of referring pages, with `*.` for any one or more labels.
    pub allowed_hosts: Vec<String>,
    /// Whether requests without `Referer` pass.
    #[serde(default)]
    pub allow_empty: bool,
}

/// Basic authentication (RFC 7617) against an htpasswd file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasicAuth {
    pub realm: String,
    /// The htpasswd file in the gateway's secret directory.
    pub users_secret_id: String,
}

/// What a rate limit counts requests by.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum RateLimitKey {
    ClientAddress,
    Host,
    Route,
    /// A request header's value; requests without it share one bucket.
    Header {
        name: String,
    },
}

/// A token bucket: `requests` per `per_seconds`, refilled continuously,
/// holding at most `burst` more.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub key: RateLimitKey,
    pub requests: u64,
    pub per_seconds: u64,
    #[serde(default)]
    pub burst: u64,
}

/// The answer to requests over a limit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitedResponse {
    pub status: u16,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// Where trusted proxies put the address of the client they forward.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum RealIpHeader {
    #[default]
    XForwardedFor,
    XRealIp,
    /// RFC 7239.
    Forwarded,
}

impl RealIpHeader {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
