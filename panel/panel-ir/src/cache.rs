//! Cache policies of the proxy cache (ADR 0043).

use crate::RouteCondition;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Required by snapshots with an enabled cache policy.
pub const PROXY_CACHE_CAPABILITY: &str = "proxy.cache";
/// The key of a policy that writes none, as nginx's `proxy_cache_key`.
pub const DEFAULT_CACHE_KEY: &str = "$scheme$host$request_uri";
/// The store's size when the configuration gives none.
pub const DEFAULT_CACHE_BYTES: u64 = 256 << 20;
/// The largest response a policy that writes none stores.
pub const DEFAULT_MOST_OBJECT_BYTES: u64 = 8 << 20;
/// Statuses a default freshness applies to: those RFC 9110 §15.1 lets
/// caches store by default, without partial and error responses.
pub const DEFAULT_TTL_STATUSES: [u16; 6] = [200, 203, 204, 300, 301, 308];

const fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What a route's responses are cached by and for how long.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    pub id: String,
    /// A disabled policy caches nothing.
    pub enabled: bool,
    /// Seconds responses with [`DEFAULT_TTL_STATUSES`] are fresh for when
    /// their origin does not say; 0 for none.
    pub ttl_seconds: u64,
    /// Request fields whose values keep responses apart, besides those the
    /// response's `Vary` names; lowercase.
    pub vary_headers: BTreeSet<String>,
    /// Seconds by status, ahead of `ttl_seconds`; 0 keeps a status out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub status_ttls: BTreeMap<u16, u64>,
    /// A template of request variables; [`DEFAULT_CACHE_KEY`] when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
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
    /// The largest response body stored; [`DEFAULT_MOST_OBJECT_BYTES`] when
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_object_bytes: Option<u64>,
    /// Whether responses carry `Cache-Status` (RFC 9211).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub status_header: bool,
}

impl CachePolicy {
    /// A policy with nothing but its identity, enabled.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            ttl_seconds: 0,
            vary_headers: BTreeSet::new(),
            status_ttls: BTreeMap::new(),
            key: None,
            honor_origin: true,
            bypass: Vec::new(),
            stale_while_revalidate_seconds: 0,
            stale_if_error_seconds: 0,
            max_object_bytes: None,
            status_header: true,
        }
    }

    /// How long a response with `status` is fresh when its origin does not
    /// say; `None` when the policy does not store it then.
    pub fn fresh_seconds(&self, status: u16) -> Option<u64> {
        match self.status_ttls.get(&status) {
            Some(0) => None,
            Some(seconds) => Some(*seconds),
            None if self.ttl_seconds > 0 && DEFAULT_TTL_STATUSES.contains(&status) => {
                Some(self.ttl_seconds)
            }
            None => None,
        }
    }

    /// Whether the policy keeps responses with `status` out altogether.
    pub fn refuses(&self, status: u16) -> bool {
        self.status_ttls.get(&status) == Some(&0)
    }

    pub fn key(&self) -> &str {
        self.key.as_deref().unwrap_or(DEFAULT_CACHE_KEY)
    }

    pub fn max_object_bytes(&self) -> u64 {
        self.max_object_bytes.unwrap_or(DEFAULT_MOST_OBJECT_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_follows_the_status_then_the_default() {
        let mut policy = CachePolicy::new("pages");
        policy.ttl_seconds = 600;
        policy.status_ttls = [(404, 30), (301, 0)].into();
        assert_eq!(policy.fresh_seconds(200), Some(600));
        assert_eq!(policy.fresh_seconds(404), Some(30));
        assert_eq!(policy.fresh_seconds(301), None);
        assert!(policy.refuses(301) && !policy.refuses(200));
        assert_eq!(policy.fresh_seconds(500), None);
        assert_eq!(policy.fresh_seconds(302), None);
        policy.ttl_seconds = 0;
        assert_eq!(policy.fresh_seconds(200), None);
        assert_eq!(policy.key(), DEFAULT_CACHE_KEY);
    }

    #[test]
    fn written_policies_read_back_with_defaults_left_out() {
        let policy: CachePolicy = serde_json::from_value(serde_json::json!({
            "id": "pages", "enabled": true, "ttl_seconds": 60, "vary_headers": []
        }))
        .unwrap();
        assert!(policy.honor_origin && policy.status_header);
        assert_eq!(policy, {
            let mut expected = CachePolicy::new("pages");
            expected.ttl_seconds = 60;
            expected
        });
        assert_eq!(
            serde_json::to_value(&policy).unwrap(),
            serde_json::json!({"id": "pages", "enabled": true, "ttl_seconds": 60, "vary_headers": []})
        );
    }
}
