use super::*;
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_ir::{
    ActiveHealthCheck, BasicAuth, CachePolicy, CapabilityRequirement, DomainSpec, HeaderPolicy,
    HealthCheckProtocol, LimitedResponse, ListenerRef, LoadBalancingPolicy, LuaPolicy,
    PassiveHealthPolicy, RateLimit, RateLimitKey, RealIpHeader, RefererRule, RetryPolicy,
    RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SecurityPolicy, SiteSpec,
    StaticContentPolicy, StrictTransportSecurity, TlsProfile, UpstreamEndpoint, UpstreamPoolSpec,
    WwwRedirect,
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
    let mut listener = ListenerRef::new("https", "0.0.0.0:443");
    listener.tls_profile_id = Some("tls-main".into());
    listener.protocols.http1 = false;
    listener.protocols.http3 = true;
    listener.reuse_port = true;
    listener.ipv6_only = Some(false);
    listener.default_site_id = Some(SiteId::new("site-main").unwrap());
    listener.trusted_proxies = ["10.0.0.0/8".into()].into_iter().collect();
    listener.real_ip_header = RealIpHeader::Forwarded;
    listener.request_head_timeout_ms = Some(15_000);
    snapshot.listeners.push(listener);
    let mut primary = DomainSpec::new(NormalizedHost::new("example.com").unwrap());
    primary.tls_profile_id = Some("tls-main".into());
    primary.primary = true;
    let mut alias = DomainSpec::new(NormalizedHost::new("*.example.net").unwrap());
    alias.enabled = false;
    alias.redirect_to_primary = true;
    let mut site = SiteSpec::new(
        SiteId::new("site-main").unwrap(),
        "main",
        vec![primary, alias],
    );
    site.listener_ids.insert("https".into());
    site.https_redirect = true;
    site.hsts = Some(StrictTransportSecurity {
        max_age_seconds: 31_536_000,
        include_subdomains: true,
        preload: false,
    });
    site.www_redirect = WwwRedirect::RemoveWww;
    site.security_policy_id = Some("security".into());
    snapshot.sites.push(site);
    let mut route = RouteSpec::new(
        RouteId::new("route-main").unwrap(),
        SiteId::new("site-main").unwrap(),
        10,
        RouteMatcher::HostPathPrefix {
            host: NormalizedHost::new("example.com").unwrap(),
            path: PathPrefix::new("/api").unwrap(),
        },
        RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("pool-main").unwrap(),
        },
    );
    route.retry_policy = Some(RetryPolicy {
        attempts: 2,
        per_try_timeout_ms: 500,
        retry_statuses: [502, 503].into_iter().collect(),
    });
    route.header_policy_id = Some("headers".into());
    route.cache_policy_id = Some("cache".into());
    route.security_policy_id = Some("security".into());
    route.lua_policy_id = Some("lua".into());
    route.name = Some("api".into());
    snapshot.routes.push(route);
    let mut redirect = RouteSpec::new(
        RouteId::new("route-redirect").unwrap(),
        SiteId::new("site-main").unwrap(),
        20,
        RouteMatcher::ExactPath {
            path: "/old".into(),
        },
        RouteAction::Redirect {
            location: "https://example.com/new".into(),
            status: 308,
            preserve_path: true,
        },
    );
    redirect.enabled = false;
    snapshot.routes.push(redirect);
    snapshot.routes.push(RouteSpec::new(
        RouteId::new("route-maintenance").unwrap(),
        SiteId::new("site-main").unwrap(),
        30,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/").unwrap(),
        },
        RouteAction::Respond {
            status: 503,
            body: Some("maintenance".into()),
            content_type: Some("text/plain; charset=utf-8".into()),
            retry_after_seconds: Some(120),
        },
    ));
    let mut endpoint = UpstreamEndpoint::new(
        EndpointId::new("origin-1").unwrap(),
        EndpointAddress::new("127.0.0.1", 8443, true).unwrap(),
    );
    endpoint.sni = Some("origin.example.com".into());
    endpoint.weight = 10;
    let mut backup = UpstreamEndpoint::new(
        EndpointId::new("origin-2").unwrap(),
        EndpointAddress::new("localhost", 80, false).unwrap(),
    );
    backup.enabled = false;
    backup.backup = true;
    backup.unix_socket = Some("/run/app.sock".into());
    let mut pool = UpstreamPoolSpec::new(
        UpstreamPoolId::new("pool-main").unwrap(),
        "primary",
        vec![endpoint, backup],
    );
    pool.load_balancing = LoadBalancingPolicy::ConsistentHash {
        key: "client_ip".into(),
    };
    pool.retry_policy = RetryPolicy {
        attempts: 3,
        per_try_timeout_ms: 750,
        retry_statuses: [500, 502].into_iter().collect(),
    };
    pool.connection.connect_timeout_ms = Some(1_000);
    pool.connection.idle_timeout_ms = Some(60_000);
    pool.connection.keepalive = false;
    pool.connection.max_connections = Some(128);
    pool.connection.http2 = true;
    pool.tls.verify_hostname = false;
    pool.tls.ca_secret_id = Some("ca".into());
    pool.tls.sni = Some("origin.example.com".into());
    pool.host_header = Some("origin.example.com".into());
    pool.health_check = Some(ActiveHealthCheck {
        protocol: HealthCheckProtocol::Http,
        path: "/healthz".into(),
        method: "HEAD".into(),
        interval_ms: 5_000,
        timeout_ms: 1_000,
        healthy_threshold: 2,
        unhealthy_threshold: 3,
        expected_statuses: [200, 204].into_iter().collect(),
        host: Some("origin.example.com".into()),
    });
    pool.passive_health = Some(PassiveHealthPolicy {
        failure_threshold: 5,
        ejection_ms: 30_000,
    });
    snapshot.upstream_pools.push(pool);
    snapshot.tls_profiles.push(TlsProfile {
        id: "tls-main".into(),
        certificate_secret_id: "cert".into(),
        private_key_secret_id: "key".into(),
        min_protocol: "TLS1.2".into(),
        max_protocol: Some("TLSv1.3".into()),
        cipher_suites: vec!["TLS13_AES_128_GCM_SHA256".into()],
        session_resumption: false,
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
        allowed_methods: ["GET".into(), "HEAD".into()].into_iter().collect(),
        denied_path_prefixes: vec!["/admin".into()],
        denied_user_agents: vec!["(?i)scanner".into()],
        referer: Some(RefererRule {
            allowed_hosts: vec!["*.example.com".into()],
            allow_empty: true,
        }),
        basic_auth: Some(BasicAuth {
            realm: "Staff".into(),
            users_secret_id: "staff.htpasswd".into(),
        }),
        max_header_bytes: Some(16_384),
        max_body_bytes: Some(1_048_576),
        body_timeout_ms: Some(30_000),
        rate_limits: vec![
            RateLimit {
                key: RateLimitKey::ClientAddress,
                requests: 10,
                per_seconds: 1,
                burst: 20,
            },
            RateLimit {
                key: RateLimitKey::Header {
                    name: "x-api-key".into(),
                },
                requests: 600,
                per_seconds: 60,
                burst: 0,
            },
        ],
        max_concurrent_requests: Some(8),
        limited_response: Some(LimitedResponse {
            status: 503,
            body: "slow down".into(),
            content_type: Some("text/plain".into()),
        }),
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

/// Messages from senders that predate the extension fields decode to the
/// IR defaults, so their canonical hash is unchanged.
#[test]
fn absent_extension_messages_decode_to_defaults() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(9));
    snapshot
        .listeners
        .push(ListenerRef::new("http", "0.0.0.0:80"));
    snapshot.upstream_pools.push(UpstreamPoolSpec::new(
        UpstreamPoolId::new("pool").unwrap(),
        "pool",
        vec![UpstreamEndpoint::new(
            EndpointId::new("node").unwrap(),
            EndpointAddress::new("127.0.0.1", 8080, false).unwrap(),
        )],
    ));
    snapshot.refresh_content_hash();
    let mut wire = encode_snapshot(&snapshot);
    wire.listeners[0].protocols = None;
    wire.upstream_pools[0].connection = None;
    wire.upstream_pools[0].tls = None;
    assert_eq!(decode_snapshot(wire).unwrap(), snapshot);
}

#[test]
fn logging_settings_round_trip() {
    use panel_ir::{
        logging::LOGGING_CAPABILITY, AccessLog, AccessLogFormat, LogFiles, LoggingPolicy,
    };
    use std::collections::{BTreeMap, BTreeSet};

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(9));
    let mut site = SiteSpec::new(SiteId::new("site").unwrap(), "site", Vec::new());
    site.access_log.enabled = Some(false);
    snapshot.sites.push(site);
    let mut route = RouteSpec::new(
        RouteId::new("route").unwrap(),
        SiteId::new("site").unwrap(),
        10,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/").unwrap(),
        },
        RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("pool").unwrap(),
        },
    );
    route.access_log = AccessLog {
        enabled: Some(true),
        format: Some(AccessLogFormat::Combined),
        fields: BTreeMap::from([("tenant".to_owned(), "$http_x_tenant".to_owned())]),
    };
    snapshot.routes.push(route);
    snapshot.logging = LoggingPolicy {
        access: AccessLog {
            format: Some(AccessLogFormat::Json),
            ..AccessLog::default()
        },
        files: LogFiles {
            max_size_bytes: 1 << 20,
            rotate_daily: false,
            keep_days: 0,
            max_files: 3,
        },
        redact_query: Some(BTreeSet::new()),
        redact_headers: BTreeSet::from(["x-api-key".to_owned()]),
    };
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(LOGGING_CAPABILITY, "1"));
    snapshot.refresh_content_hash();

    let decoded = decode_snapshot(encode_snapshot(&snapshot)).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn route_conditions_round_trip_and_unknown_kinds_are_refused() {
    use panel_ir::{RouteCondition, ValueTest};

    let test = |value: &str| ValueTest::Equals {
        value: value.into(),
        ignore_case: true,
    };
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(10));
    let mut route = RouteSpec::new(
        RouteId::new("route").unwrap(),
        SiteId::new("site").unwrap(),
        10,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/api").unwrap(),
        },
        RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("pool").unwrap(),
        },
    );
    route.conditions = vec![
        RouteCondition::Method {
            methods: vec!["GET".into(), "HEAD".into()],
        },
        RouteCondition::Host {
            hosts: vec![NormalizedHost::new("*.shop.example").unwrap()],
        },
        RouteCondition::Header {
            name: "x-env".into(),
            test: test("staging"),
        },
        RouteCondition::Query {
            name: "debug".into(),
            test: ValueTest::Present,
        },
        RouteCondition::Cookie {
            name: "beta".into(),
            test: ValueTest::Regex {
                pattern: "^on$".into(),
                ignore_case: false,
            },
        },
        RouteCondition::Client {
            networks: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
        },
        RouteCondition::UserAgent {
            test: ValueTest::Contains {
                value: "bot".into(),
                ignore_case: true,
            },
        },
        RouteCondition::Referer {
            test: ValueTest::Prefix {
                value: "https://shop.example/".into(),
                ignore_case: false,
            },
        },
        RouteCondition::ContentType {
            types: vec!["application/json".into(), "text/*".into()],
        },
        RouteCondition::Any {
            conditions: vec![
                RouteCondition::Header {
                    name: "x-canary".into(),
                    test: ValueTest::Suffix {
                        value: "1".into(),
                        ignore_case: false,
                    },
                },
                RouteCondition::All {
                    conditions: vec![RouteCondition::Cookie {
                        name: "canary".into(),
                        test: ValueTest::Absent,
                    }],
                },
            ],
        },
        RouteCondition::Not {
            condition: Box::new(RouteCondition::Client {
                networks: vec!["192.0.2.0/24".into()],
            }),
        },
    ];
    snapshot.routes.push(route);
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire;
    unknown.routes[0].conditions[0].kind = None;
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("does not know"), "{refused}");
}
