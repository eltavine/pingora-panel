use super::*;
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_ir::{
    CachePolicy, CapabilityRequirement, DomainSpec, HeaderPolicy, ListenerRef, LoadBalancingPolicy,
    LuaPolicy, RetryPolicy, RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SecurityPolicy,
    SiteSpec, StaticContentPolicy, TlsProfile, UpstreamEndpoint, UpstreamPoolSpec,
};

#[test]
fn empty_snapshot_round_trips_without_transport_types_leaking() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(7));
    let decoded = decode_snapshot(encode_snapshot(&snapshot)).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn populated_snapshot_round_trips_additive_v1_fields() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(8));
    snapshot.listeners.push(ListenerRef {
        id: "https".into(),
        address: "0.0.0.0:443".into(),
        tls_profile_id: Some("tls-main".into()),
    });
    snapshot.sites.push(SiteSpec {
        id: SiteId::new("site-main").unwrap(),
        name: "main".into(),
        enabled: true,
        domains: vec![DomainSpec {
            host: NormalizedHost::new("example.com").unwrap(),
            tls_profile_id: Some("tls-main".into()),
        }],
    });
    snapshot.routes.push(RouteSpec {
        id: RouteId::new("route-main").unwrap(),
        site_id: SiteId::new("site-main").unwrap(),
        priority: 10,
        enabled: true,
        matcher: RouteMatcher::HostPathPrefix {
            host: NormalizedHost::new("example.com").unwrap(),
            path: PathPrefix::new("/api").unwrap(),
        },
        action: RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("pool-main").unwrap(),
        },
        retry_policy: Some(RetryPolicy {
            attempts: 2,
            per_try_timeout_ms: 500,
            retry_statuses: [502, 503].into_iter().collect(),
        }),
        header_policy_id: Some("headers".into()),
        cache_policy_id: Some("cache".into()),
        security_policy_id: Some("security".into()),
        lua_policy_id: Some("lua".into()),
    });
    snapshot.upstream_pools.push(UpstreamPoolSpec {
        id: UpstreamPoolId::new("pool-main").unwrap(),
        name: "primary".into(),
        endpoints: vec![UpstreamEndpoint {
            id: EndpointId::new("origin-1").unwrap(),
            address: EndpointAddress::new("127.0.0.1", 8443, true).unwrap(),
            sni: Some("origin.example.com".into()),
            weight: 10,
        }],
        load_balancing: LoadBalancingPolicy::ConsistentHash {
            key: "client-ip".into(),
        },
        retry_policy: RetryPolicy {
            attempts: 3,
            per_try_timeout_ms: 750,
            retry_statuses: [500, 502].into_iter().collect(),
        },
    });
    snapshot.tls_profiles.push(TlsProfile {
        id: "tls-main".into(),
        certificate_secret_id: "cert".into(),
        private_key_secret_id: "key".into(),
        min_protocol: "TLS1.2".into(),
        alpn: ["h2".into(), "http/1.1".into()].into_iter().collect(),
    });
    snapshot.header_policies.push(HeaderPolicy {
        id: "headers".into(),
        request_set: [("x-request".into(), "1".into())].into_iter().collect(),
        request_remove: ["x-remove".into()].into_iter().collect(),
        response_set: [("x-response".into(), "1".into())].into_iter().collect(),
        response_remove: ["server".into()].into_iter().collect(),
    });
    snapshot.static_content.push(StaticContentPolicy {
        id: "static".into(),
        root: "/srv/www".into(),
        index_files: vec!["index.html".into()],
        spa_fallback: true,
    });
    snapshot.cache_policies.push(CachePolicy {
        id: "cache".into(),
        enabled: true,
        ttl_seconds: 60,
        vary_headers: ["accept-encoding".into()].into_iter().collect(),
    });
    snapshot.security_policies.push(SecurityPolicy {
        id: "security".into(),
        allowed_cidrs: ["10.0.0.0/8".into()].into_iter().collect(),
        denied_cidrs: ["10.1.0.0/16".into()].into_iter().collect(),
        request_rate_per_second: Some(100),
    });
    snapshot.lua_policies.push(LuaPolicy {
        id: "lua".into(),
        script_secret_id: "script".into(),
        instruction_limit: 10_000,
        timeout_ms: 10,
        memory_limit_bytes: 1_048_576,
        capabilities: ["request.headers".into()].into_iter().collect(),
    });
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new("upstream.https", "1"));
    snapshot.refresh_content_hash();

    let decoded = decode_snapshot(encode_snapshot(&snapshot)).unwrap();
    assert_eq!(decoded, snapshot);
}
