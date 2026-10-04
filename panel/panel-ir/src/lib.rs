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

pub use logging::{AccessLog, AccessLogFormat, LogFiles, LoggingPolicy};
pub use security::{
    BasicAuth, LimitedResponse, RateLimit, RateLimitKey, RealIpHeader, RefererRule, SecurityPolicy,
    REQUEST_HEAD_TIMEOUT_CAPABILITY, REQUEST_SECURITY_CAPABILITY, TRUSTED_PROXIES_CAPABILITY,
};

pub mod logging;
pub mod security;
pub mod template;
pub mod tls;

pub const IR_SCHEMA_VERSION: &str = "pingora.panel.ir/v1alpha1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "LoggingPolicy::is_default")]
    pub logging: LoggingPolicy,
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
    #[serde(skip_serializing_if = "LoggingPolicy::is_default")]
    logging: &'a LoggingPolicy,
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
            logging: LoggingPolicy::default(),
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
            logging: &self.logging,
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

// Fields added after the first schema release default to values that are not
// serialized, so snapshots written before them keep their canonical hash.
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

const fn enabled() -> bool {
    true
}

/// A fixed listening socket. `address` is an `ip:port` socket address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerRef {
    pub id: String,
    pub address: String,
    pub tls_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "ListenerProtocols::is_default")]
    pub protocols: ListenerProtocols,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reuse_port: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_only: Option<bool>,
    /// Serves requests whose host matches no site; without it they are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_site_id: Option<SiteId>,
    /// Proxies in these networks name the client in `real_ip_header`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub trusted_proxies: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "RealIpHeader::is_default")]
    pub real_ip_header: RealIpHeader,
    /// The longest a client may take to send a request head: from the
    /// connection's start for its first request, and from the end of the
    /// previous response for later ones. The gateway's own default applies
    /// when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_head_timeout_ms: Option<u64>,
}

impl ListenerRef {
    pub fn new(id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            address: address.into(),
            tls_profile_id: None,
            protocols: ListenerProtocols::default(),
            reuse_port: false,
            ipv6_only: None,
            default_site_id: None,
            trusted_proxies: BTreeSet::new(),
            real_ip_header: RealIpHeader::default(),
            request_head_timeout_ms: None,
        }
    }
}

/// HTTP versions accepted on a listener. With TLS, HTTP/2 is negotiated with
/// ALPN; without TLS it is accepted with prior knowledge (h2c).
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerProtocols {
    pub http1: bool,
    pub http2: bool,
    pub http3: bool,
}

impl Default for ListenerProtocols {
    fn default() -> Self {
        Self {
            http1: true,
            http2: true,
            http3: false,
        }
    }
}

impl ListenerProtocols {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteSpec {
    pub id: SiteId,
    pub name: String,
    pub enabled: bool,
    pub domains: Vec<DomainSpec>,
    /// Listeners serving this site; empty means every listener.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub listener_ids: BTreeSet<String>,
    /// Redirects cleartext requests to the site's HTTPS listener.
    #[serde(default, skip_serializing_if = "is_false")]
    pub https_redirect: bool,
    #[serde(default, skip_serializing_if = "WwwRedirect::is_none")]
    pub www_redirect: WwwRedirect,
    /// Sent with every HTTPS response for the site's hosts (RFC 6797).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsts: Option<StrictTransportSecurity>,
    /// Every request for the site passes this policy first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "AccessLog::is_unset")]
    pub access_log: AccessLog,
}

/// An HTTP Strict Transport Security policy (RFC 6797 §6.1).
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrictTransportSecurity {
    /// How long browsers keep using HTTPS only; zero makes them forget it.
    pub max_age_seconds: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_subdomains: bool,
    /// Consents to inclusion in browsers' preload lists.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preload: bool,
}

impl StrictTransportSecurity {
    /// The `Strict-Transport-Security` header value.
    pub fn header_value(&self) -> String {
        let mut value = format!("max-age={}", self.max_age_seconds);
        if self.include_subdomains {
            value.push_str("; includeSubDomains");
        }
        if self.preload {
            value.push_str("; preload");
        }
        value
    }
}

impl SiteSpec {
    pub fn new(id: SiteId, name: impl Into<String>, domains: Vec<DomainSpec>) -> Self {
        Self {
            id,
            name: name.into(),
            enabled: true,
            domains,
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            hsts: None,
            security_policy_id: None,
            access_log: AccessLog::default(),
        }
    }
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WwwRedirect {
    #[default]
    None,
    /// `example.com` redirects to `www.example.com`.
    AddWww,
    /// `www.example.com` redirects to `example.com`.
    RemoveWww,
}

impl WwwRedirect {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSpec {
    pub host: NormalizedHost,
    pub tls_profile_id: Option<String>,
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The canonical name used as the target of alias and `www` redirects.
    #[serde(default, skip_serializing_if = "is_false")]
    pub primary: bool,
    /// An alias that redirects to the primary domain instead of serving the site.
    #[serde(default, skip_serializing_if = "is_false")]
    pub redirect_to_primary: bool,
}

impl DomainSpec {
    pub fn new(host: NormalizedHost) -> Self {
        Self {
            host,
            tls_profile_id: None,
            enabled: true,
            primary: false,
            redirect_to_primary: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "AccessLog::is_unset")]
    pub access_log: AccessLog,
}

impl RouteSpec {
    pub fn new(
        id: RouteId,
        site_id: SiteId,
        priority: u32,
        matcher: RouteMatcher,
        action: RouteAction,
    ) -> Self {
        Self {
            id,
            site_id,
            priority,
            enabled: true,
            matcher,
            action,
            retry_policy: None,
            header_policy_id: None,
            cache_policy_id: None,
            security_policy_id: None,
            lua_policy_id: None,
            name: None,
            access_log: AccessLog::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub enum RouteAction {
    Proxy {
        upstream_pool_id: UpstreamPoolId,
    },
    Static {
        policy_id: String,
    },
    Redirect {
        /// A [template](template), evaluated per request.
        location: String,
        status: u16,
        /// Appends the request path and query to `location`.
        #[serde(default, skip_serializing_if = "is_false")]
        preserve_path: bool,
    },
    Respond {
        status: u16,
        /// A [template](template), evaluated per request.
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_seconds: Option<u32>,
    },
}

impl RouteAction {
    pub fn redirect(location: impl Into<String>, status: u16) -> Self {
        Self::Redirect {
            location: location.into(),
            status,
            preserve_path: false,
        }
    }

    pub fn respond(status: u16, body: Option<String>) -> Self {
        Self::Respond {
            status,
            body,
            content_type: None,
            retry_after_seconds: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamPoolSpec {
    pub id: UpstreamPoolId,
    pub name: String,
    pub endpoints: Vec<UpstreamEndpoint>,
    pub load_balancing: LoadBalancingPolicy,
    pub retry_policy: RetryPolicy,
    #[serde(default, skip_serializing_if = "UpstreamConnectionPolicy::is_default")]
    pub connection: UpstreamConnectionPolicy,
    #[serde(default, skip_serializing_if = "UpstreamTlsPolicy::is_default")]
    pub tls: UpstreamTlsPolicy,
    /// Replaces the `Host` header sent upstream; the client's host is kept otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<ActiveHealthCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passive_health: Option<PassiveHealthPolicy>,
}

impl UpstreamPoolSpec {
    pub fn new(
        id: UpstreamPoolId,
        name: impl Into<String>,
        endpoints: Vec<UpstreamEndpoint>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            endpoints,
            load_balancing: LoadBalancingPolicy::RoundRobin,
            retry_policy: RetryPolicy::none(),
            connection: UpstreamConnectionPolicy::default(),
            tls: UpstreamTlsPolicy::default(),
            host_header: None,
            health_check: None,
            passive_health: None,
        }
    }
}

/// Timeouts are in milliseconds; `None` uses the engine default.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConnectionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_timeout_ms: Option<u64>,
    /// How long an idle pooled connection is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_ms: Option<u64>,
    /// Reuses upstream connections from the pool.
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub keepalive: bool,
    /// Concurrent requests per endpoint; saturated endpoints are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<u32>,
    /// Speaks HTTP/2 to TLS upstreams that negotiate it with ALPN.
    #[serde(default, skip_serializing_if = "is_false")]
    pub http2: bool,
}

impl Default for UpstreamConnectionPolicy {
    fn default() -> Self {
        Self {
            connect_timeout_ms: None,
            read_timeout_ms: None,
            write_timeout_ms: None,
            idle_timeout_ms: None,
            keepalive: true,
            max_connections: None,
            http2: false,
        }
    }
}

impl UpstreamConnectionPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTlsPolicy {
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub verify_certificate: bool,
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub verify_hostname: bool,
    /// Trust anchors replacing the system roots, as a PEM bundle secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_secret_id: Option<String>,
    /// Server name for endpoints that do not set their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
}

impl Default for UpstreamTlsPolicy {
    fn default() -> Self {
        Self {
            verify_certificate: true,
            verify_hostname: true,
            ca_secret_id: None,
            sni: None,
        }
    }
}

impl UpstreamTlsPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCheckProtocol {
    Http,
    Tcp,
}

/// Probes every endpoint; thresholds count consecutive results.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveHealthCheck {
    pub protocol: HealthCheckProtocol,
    pub path: String,
    pub method: String,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub healthy_threshold: u32,
    pub unhealthy_threshold: u32,
    /// Statuses meaning healthy; empty means any 2xx or 3xx.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub expected_statuses: BTreeSet<u16>,
    /// `Host` sent with HTTP probes; the endpoint address otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Ejects an endpoint after consecutive proxy failures for `ejection_ms`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassiveHealthPolicy {
    pub failure_threshold: u32,
    pub ejection_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamEndpoint {
    pub id: EndpointId,
    pub address: EndpointAddress,
    pub sni: Option<String>,
    pub weight: u32,
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Receives traffic only while no primary endpoint is available.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backup: bool,
    /// Connects to a Unix domain socket instead of `address`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_socket: Option<String>,
}

impl UpstreamEndpoint {
    pub fn new(id: EndpointId, address: EndpointAddress) -> Self {
        Self {
            id,
            address,
            sni: None,
            weight: 1,
            enabled: true,
            backup: false,
            unix_socket: None,
        }
    }
}

/// Consistent hash keys are `client_ip`, `uri`, `header:<name>` or `cookie:<name>`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum LoadBalancingPolicy {
    RoundRobin,
    Random,
    ConsistentHash { key: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub per_try_timeout_ms: u64,
    pub retry_statuses: BTreeSet<u16>,
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self {
            attempts: 0,
            per_try_timeout_ms: 0,
            retry_statuses: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderPolicy {
    pub id: String,
    pub request_set: BTreeMap<String, String>,
    pub request_remove: BTreeSet<String>,
    pub response_set: BTreeMap<String, String>,
    pub response_remove: BTreeSet<String>,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsProfile {
    pub id: String,
    pub certificate_secret_id: String,
    pub private_key_secret_id: String,
    pub min_protocol: String,
    /// The newest TLS version accepted; the newest the engine supports when
    /// absent. Like the cipher suites and session resumption, it applies to
    /// listeners that use this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_protocol: Option<String>,
    /// IANA names of the cipher suites accepted, from [`tls::CIPHER_SUITES`];
    /// empty accepts the engine's defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cipher_suites: Vec<String>,
    /// Off refuses resuming sessions by session IDs and tickets.
    #[serde(default = "enabled", skip_serializing_if = "is_true")]
    pub session_resumption: bool,
    /// ALPN protocol IDs a listener using this profile offers, narrowing the
    /// listener's enabled protocols; empty offers all of them. A profile chosen
    /// by SNI for a domain does not change its listener's offer.
    pub alpn: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticContentPolicy {
    pub id: String,
    pub root: String,
    pub index_files: Vec<String>,
    pub spa_fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    pub id: String,
    pub enabled: bool,
    pub ttl_seconds: u64,
    pub vary_headers: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

    /// Snapshots written before a field existed must keep their hash, so
    /// defaulted fields are absent from canonical bytes.
    #[test]
    fn defaulted_fields_do_not_change_canonical_bytes() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot
            .listeners
            .push(ListenerRef::new("http", "0.0.0.0:80"));
        snapshot.sites.push(SiteSpec::new(
            SiteId::new("site").unwrap(),
            "site",
            vec![DomainSpec::new(NormalizedHost::new("example.com").unwrap())],
        ));
        snapshot.routes.push(RouteSpec::new(
            RouteId::new("route").unwrap(),
            SiteId::new("site").unwrap(),
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").unwrap(),
            },
            RouteAction::respond(204, None),
        ));
        snapshot.upstream_pools.push(UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![UpstreamEndpoint::new(
                EndpointId::new("node").unwrap(),
                EndpointAddress::new("127.0.0.1", 8080, false).unwrap(),
            )],
        ));
        assert_eq!(
            String::from_utf8(snapshot.canonical_bytes()).unwrap(),
            concat!(
                r#"{"schema_version":"pingora.panel.ir/v1alpha1","revision_id":1,"#,
                r#""listeners":[{"id":"http","address":"0.0.0.0:80","tls_profile_id":null}],"#,
                r#""sites":[{"id":"site","name":"site","enabled":true,"#,
                r#""domains":[{"host":"example.com","tls_profile_id":null}]}],"#,
                r#""routes":[{"id":"route","site_id":"site","priority":1,"enabled":true,"#,
                r#""matcher":{"kind":"path_prefix","path":"/"},"#,
                r#""action":{"kind":"respond","status":204,"body":null},"#,
                r#""retry_policy":null,"header_policy_id":null,"cache_policy_id":null,"#,
                r#""security_policy_id":null,"lua_policy_id":null}],"#,
                r#""upstream_pools":[{"id":"pool","name":"pool","endpoints":[{"id":"node","#,
                r#""address":{"host":"127.0.0.1","port":8080,"tls":false},"sni":null,"weight":1}],"#,
                r#""load_balancing":"round_robin","#,
                r#""retry_policy":{"attempts":0,"per_try_timeout_ms":0,"retry_statuses":[]}}],"#,
                r#""tls_profiles":[],"header_policies":[],"static_content":[],"cache_policies":[],"#,
                r#""security_policies":[],"lua_policies":[],"required_capabilities":[]}"#
            )
        );
    }

    #[test]
    fn extension_fields_round_trip() {
        let mut listener = ListenerRef::new("https", "[::]:443");
        listener.protocols.http3 = true;
        listener.reuse_port = true;
        listener.default_site_id = Some(SiteId::new("site").unwrap());
        let mut pool = UpstreamPoolSpec::new(UpstreamPoolId::new("pool").unwrap(), "pool", vec![]);
        pool.connection.keepalive = false;
        pool.tls.verify_hostname = false;
        pool.health_check = Some(ActiveHealthCheck {
            protocol: HealthCheckProtocol::Http,
            path: "/healthz".into(),
            method: "GET".into(),
            interval_ms: 5_000,
            timeout_ms: 1_000,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            expected_statuses: BTreeSet::from([200]),
            host: None,
        });
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(2));
        snapshot.listeners.push(listener);
        snapshot.upstream_pools.push(pool);
        snapshot.refresh_content_hash();
        let decoded: RuntimeSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded, snapshot);
        assert!(decoded.has_valid_content_hash());
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
