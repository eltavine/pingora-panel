#![forbid(unsafe_code)]

//! Versioned engine-neutral runtime representation.
//!
//! Collections which affect canonical output use `BTreeMap`/`BTreeSet`. The declared
//! `content_hash` is excluded from canonical bytes to avoid a self-referential digest.

use panel_domain::{
    ContentHash, EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId,
    SiteId, UpstreamPoolId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const IR_SCHEMA_VERSION: &str = "pingora.panel.ir/v1alpha1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CapabilityRequirement {
    pub name: String,
    pub version: String,
}

impl CapabilityRequirement {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub schema_version: String,
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub listeners: Vec<ListenerRef>,
    pub sites: Vec<SiteSpec>,
    pub routes: Vec<RouteSpec>,
    pub upstream_pools: Vec<UpstreamPoolSpec>,
    pub tls_profiles: Vec<TlsProfile>,
    pub header_policies: Vec<HeaderPolicy>,
    pub static_content: Vec<StaticContentPolicy>,
    pub cache_policies: Vec<CachePolicy>,
    pub security_policies: Vec<SecurityPolicy>,
    pub lua_policies: Vec<LuaPolicy>,
    pub required_capabilities: Vec<CapabilityRequirement>,
}

#[derive(Serialize)]
struct CanonicalSnapshot<'a> {
    schema_version: &'a str,
    revision_id: &'a RevisionId,
    listeners: &'a [ListenerRef],
    sites: &'a [SiteSpec],
    routes: &'a [RouteSpec],
    upstream_pools: &'a [UpstreamPoolSpec],
    tls_profiles: &'a [TlsProfile],
    header_policies: &'a [HeaderPolicy],
    static_content: &'a [StaticContentPolicy],
    cache_policies: &'a [CachePolicy],
    security_policies: &'a [SecurityPolicy],
    lua_policies: &'a [LuaPolicy],
    required_capabilities: &'a [CapabilityRequirement],
}

impl RuntimeSnapshot {
    pub fn empty(revision_id: RevisionId) -> Self {
        let mut snapshot = Self {
            schema_version: IR_SCHEMA_VERSION.to_string(),
            revision_id,
            content_hash: ContentHash::from_bytes(&[]),
            listeners: Vec::new(),
            sites: Vec::new(),
            routes: Vec::new(),
            upstream_pools: Vec::new(),
            tls_profiles: Vec::new(),
            header_policies: Vec::new(),
            static_content: Vec::new(),
            cache_policies: Vec::new(),
            security_policies: Vec::new(),
            lua_policies: Vec::new(),
            required_capabilities: Vec::new(),
        };
        snapshot.refresh_content_hash();
        snapshot
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut listeners = self.listeners.clone();
        listeners.sort_by(|left, right| left.id.cmp(&right.id));
        let mut sites = self.sites.clone();
        sites.sort_by(|left, right| left.id.cmp(&right.id));
        for site in &mut sites {
            site.domains
                .sort_by(|left, right| left.host.cmp(&right.host));
        }
        let mut routes = self.routes.clone();
        routes.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut upstream_pools = self.upstream_pools.clone();
        upstream_pools.sort_by(|left, right| left.id.cmp(&right.id));
        for pool in &mut upstream_pools {
            pool.endpoints.sort_by(|left, right| left.id.cmp(&right.id));
        }
        let mut tls_profiles = self.tls_profiles.clone();
        tls_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        let mut header_policies = self.header_policies.clone();
        header_policies.sort_by(|left, right| left.id.cmp(&right.id));
        let mut static_content = self.static_content.clone();
        static_content.sort_by(|left, right| left.id.cmp(&right.id));
        let mut cache_policies = self.cache_policies.clone();
        cache_policies.sort_by(|left, right| left.id.cmp(&right.id));
        let mut security_policies = self.security_policies.clone();
        security_policies.sort_by(|left, right| left.id.cmp(&right.id));
        let mut lua_policies = self.lua_policies.clone();
        lua_policies.sort_by(|left, right| left.id.cmp(&right.id));
        let mut required_capabilities = self.required_capabilities.clone();
        required_capabilities.sort();
        let canonical = CanonicalSnapshot {
            schema_version: &self.schema_version,
            revision_id: &self.revision_id,
            listeners: &listeners,
            sites: &sites,
            routes: &routes,
            upstream_pools: &upstream_pools,
            tls_profiles: &tls_profiles,
            header_policies: &header_policies,
            static_content: &static_content,
            cache_policies: &cache_policies,
            security_policies: &security_policies,
            lua_policies: &lua_policies,
            required_capabilities: &required_capabilities,
        };
        serde_json::to_vec(&canonical).expect("IR canonical values are always serializable")
    }

    pub fn content_hash(&self) -> ContentHash {
        ContentHash::from_bytes(&self.canonical_bytes())
    }

    pub fn refresh_content_hash(&mut self) {
        self.content_hash = self.content_hash();
    }

    pub fn has_valid_content_hash(&self) -> bool {
        self.content_hash == self.content_hash()
    }

    pub fn required_capabilities(&self) -> &[CapabilityRequirement] {
        &self.required_capabilities
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListenerRef {
    pub id: String,
    pub address: String,
    pub tls_profile_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SiteSpec {
    pub id: SiteId,
    pub name: String,
    pub enabled: bool,
    pub domains: Vec<DomainSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainSpec {
    pub host: NormalizedHost,
    pub tls_profile_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteSpec {
    pub id: RouteId,
    pub site_id: SiteId,
    pub priority: u32,
    pub enabled: bool,
    pub matcher: RouteMatcher,
    pub action: RouteAction,
    pub retry_policy: Option<RetryPolicy>,
    pub header_policy_id: Option<String>,
    pub cache_policy_id: Option<String>,
    pub security_policy_id: Option<String>,
    pub lua_policy_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteMatcher {
    Host {
        host: NormalizedHost,
    },
    PathPrefix {
        path: PathPrefix,
    },
    HostPathPrefix {
        host: NormalizedHost,
        path: PathPrefix,
    },
    ExactPath {
        path: String,
    },
    Glob {
        pattern: String,
    },
    Regex {
        pattern: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteAction {
    Proxy { upstream_pool_id: UpstreamPoolId },
    Static { policy_id: String },
    Redirect { location: String, status: u16 },
    Respond { status: u16, body: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpstreamPoolSpec {
    pub id: UpstreamPoolId,
    pub name: String,
    pub endpoints: Vec<UpstreamEndpoint>,
    pub load_balancing: LoadBalancingPolicy,
    pub retry_policy: RetryPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpstreamEndpoint {
    pub id: EndpointId,
    pub address: EndpointAddress,
    pub sni: Option<String>,
    pub weight: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancingPolicy {
    RoundRobin,
    Random,
    ConsistentHash { key: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub per_try_timeout_ms: u64,
    pub retry_statuses: BTreeSet<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeaderPolicy {
    pub id: String,
    pub request_set: BTreeMap<String, String>,
    pub request_remove: BTreeSet<String>,
    pub response_set: BTreeMap<String, String>,
    pub response_remove: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TlsProfile {
    pub id: String,
    pub certificate_secret_id: String,
    pub private_key_secret_id: String,
    pub min_protocol: String,
    pub alpn: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StaticContentPolicy {
    pub id: String,
    pub root: String,
    pub index_files: Vec<String>,
    pub spa_fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CachePolicy {
    pub id: String,
    pub enabled: bool,
    pub ttl_seconds: u64,
    pub vary_headers: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub id: String,
    pub allowed_cidrs: BTreeSet<String>,
    pub denied_cidrs: BTreeSet<String>,
    pub request_rate_per_second: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LuaPolicy {
    pub id: String,
    pub script_secret_id: String,
    pub instruction_limit: u64,
    pub timeout_ms: u64,
    pub memory_limit_bytes: u64,
    pub capabilities: BTreeSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_and_hash_are_stable() {
        let left = RuntimeSnapshot::empty(RevisionId::new(1));
        let right = RuntimeSnapshot::empty(RevisionId::new(1));
        assert_eq!(left.canonical_bytes(), right.canonical_bytes());
        assert_eq!(left.content_hash(), right.content_hash());
        assert!(left.has_valid_content_hash());
    }

    #[test]
    fn revision_and_field_changes_change_hash() {
        let first = RuntimeSnapshot::empty(RevisionId::new(1));
        let second = RuntimeSnapshot::empty(RevisionId::new(2));
        let mut third = first.clone();
        third
            .required_capabilities
            .push(CapabilityRequirement::new("route.host", "1"));
        third.refresh_content_hash();
        assert_ne!(first.content_hash(), second.content_hash());
        assert_ne!(first.content_hash(), third.content_hash());
    }

    #[test]
    fn ordered_maps_ignore_insertion_order() {
        let mut left = BTreeMap::new();
        left.insert("z".to_string(), "last".to_string());
        left.insert("a".to_string(), "first".to_string());
        let mut right = BTreeMap::new();
        right.insert("a".to_string(), "first".to_string());
        right.insert("z".to_string(), "last".to_string());
        assert_eq!(
            serde_json::to_vec(&left).unwrap(),
            serde_json::to_vec(&right).unwrap()
        );
    }

    #[test]
    fn resource_order_does_not_change_canonical_hash() {
        let mut left = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut right = RuntimeSnapshot::empty(RevisionId::new(1));
        left.required_capabilities
            .push(CapabilityRequirement::new("z", "1"));
        left.required_capabilities
            .push(CapabilityRequirement::new("a", "1"));
        right
            .required_capabilities
            .push(CapabilityRequirement::new("a", "1"));
        right
            .required_capabilities
            .push(CapabilityRequirement::new("z", "1"));
        left.refresh_content_hash();
        right.refresh_content_hash();
        assert_eq!(left.canonical_bytes(), right.canonical_bytes());
        assert_eq!(left.content_hash(), right.content_hash());
    }
}
