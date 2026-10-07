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
        ..RetryPolicy::none()
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
        ..RetryPolicy::none()
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
        ..HeaderPolicy::default()
    });
    snapshot.static_content.push(StaticContentPolicy {
        id: "static".into(),
        root: "/srv/www".into(),
        index_files: vec!["index.html".into()],
        spa_fallback: true,
        listing: panel_ir::DirectoryListing::Json,
        media_types: [("wasm".to_owned(), "application/wasm".to_owned())].into(),
        default_type: Some("text/plain".into()),
        cache: vec![
            panel_ir::StaticCacheRule {
                extensions: ["css".to_owned(), "js".to_owned()].into(),
                max_age_seconds: Some(31_536_000),
                immutable: true,
            },
            panel_ir::StaticCacheRule::default(),
        ],
    });
    snapshot.cache_policies.push(CachePolicy {
        ttl_seconds: 60,
        vary_headers: ["accept-encoding".into()].into_iter().collect(),
        status_ttls: [(404, 30), (301, 0)].into(),
        key: Some("$host$uri".into()),
        honor_origin: false,
        bypass: vec![panel_ir::RouteCondition::Cookie {
            name: "session".into(),
            test: panel_ir::ValueTest::Present,
        }],
        stale_while_revalidate_seconds: 10,
        stale_if_error_seconds: 300,
        max_object_bytes: Some(1 << 20),
        status_header: false,
        ..CachePolicy::new("cache")
    });
    snapshot.cache_max_bytes = Some(64 << 20);
    snapshot.sites[0].cache_policy_id = Some("cache".into());
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

#[test]
fn rewrites_round_trip_and_unknown_kinds_and_flags_are_refused() {
    use panel_ir::{RewriteFlag, RewriteRule};

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(12));
    let mut site = SiteSpec::new(SiteId::new("site").unwrap(), "site", Vec::new());
    site.rewrites = vec![RewriteRule::Rewrite {
        pattern: "^/old/(?<rest>.*)$".into(),
        replacement: "/new/$rest".into(),
        flag: RewriteFlag::Last,
    }];
    snapshot.sites.push(site);
    let mut route = RouteSpec::new(
        RouteId::new("route").unwrap(),
        SiteId::new("site").unwrap(),
        10,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/api").unwrap(),
        },
        RouteAction::InternalRedirect {
            target: "/fallback$uri".into(),
        },
    );
    route.rewrites = vec![
        RewriteRule::StripPrefix {
            prefix: "/api".into(),
        },
        RewriteRule::AddPrefix {
            prefix: "/v2".into(),
        },
        RewriteRule::SetUri {
            template: "/index.php?q=$uri".into(),
        },
        RewriteRule::Rewrite {
            pattern: "^/a$".into(),
            replacement: "https://shop.example/a".into(),
            flag: RewriteFlag::Permanent,
        },
        RewriteRule::Rewrite {
            pattern: "^/b$".into(),
            replacement: "/c".into(),
            flag: RewriteFlag::None,
        },
    ];
    route.internal = true;
    snapshot.routes.push(route);
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire.clone();
    unknown.routes[0].rewrites[0].kind = None;
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("does not know"), "{refused}");

    let mut unknown = wire;
    if let Some(panel_contracts::gateway::v1::rewrite_rule::Kind::Rewrite(rule)) =
        unknown.sites[0].rewrites[0].kind.as_mut()
    {
        rule.flag = 42;
    }
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("rewrite flag 42"), "{refused}");
}

#[test]
fn error_pages_and_maintenance_round_trip_and_unknown_kinds_are_refused() {
    use panel_ir::{ErrorPage, ErrorPages, ErrorResponse, Maintenance};
    use std::collections::BTreeSet;

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(13));
    let mut site = SiteSpec::new(SiteId::new("site").unwrap(), "site", Vec::new());
    site.error_pages = ErrorPages {
        pages: vec![
            ErrorPage {
                statuses: BTreeSet::from([502, 503]),
                response: ErrorResponse::Body {
                    body: "<h1>Back soon, $host</h1>".into(),
                    content_type: None,
                },
                status: None,
            },
            ErrorPage {
                statuses: BTreeSet::from([404]),
                response: ErrorResponse::File {
                    path: "errors/404.html".into(),
                },
                status: Some(200),
            },
        ],
        intercept: true,
    };
    site.maintenance = Some(Maintenance {
        body: Some(String::new()),
        retry_after_seconds: Some(600),
        allow: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
        ..Maintenance::default()
    });
    snapshot.sites.push(site);
    let mut route = RouteSpec::new(
        RouteId::new("route").unwrap(),
        SiteId::new("site").unwrap(),
        10,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/old").unwrap(),
        },
        RouteAction::respond(410, None),
    );
    route.error_pages = Some(ErrorPages {
        pages: vec![ErrorPage {
            statuses: BTreeSet::from([410]),
            response: ErrorResponse::Redirect {
                location: "https://shop.example/".into(),
                status: 301,
            },
            status: None,
        }],
        intercept: false,
    });
    snapshot.routes.push(route.clone());
    route.id = RouteId::new("bare").unwrap();
    route.error_pages = Some(ErrorPages::default());
    snapshot.routes.push(route);
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire;
    unknown.sites[0].error_pages.as_mut().unwrap().pages[0].response = None;
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("does not know"), "{refused}");
}

#[test]
fn http_policies_round_trip_and_unknown_codings_are_refused() {
    use panel_ir::{
        CompressionAlgorithm, CompressionPolicy, CorsPolicy, HeaderField, ServerHeader,
    };

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(11));
    let mut site = SiteSpec::new(SiteId::new("site").unwrap(), "site", Vec::new());
    site.header_policy_id = Some("api".into());
    snapshot.sites.push(site);
    snapshot.header_policies.push(HeaderPolicy {
        id: "api".into(),
        request_add: vec![HeaderField {
            name: "x-tenant".into(),
            value: "$host".into(),
        }],
        response_add: vec![HeaderField {
            name: "link".into(),
            value: "</app.css>; rel=preload".into(),
        }],
        server: ServerHeader::Replace {
            value: "shop".into(),
        },
        cors: Some(CorsPolicy {
            allowed_origins: vec!["https://*.shop.example".into()],
            allowed_methods: vec!["PUT".into()],
            allowed_headers: vec!["x-api-key".into()],
            exposed_headers: vec!["x-request-id".into()],
            allow_credentials: true,
            max_age_seconds: Some(600),
        }),
        compression: Some(CompressionPolicy {
            algorithms: [CompressionAlgorithm::Gzip, CompressionAlgorithm::Brotli]
                .into_iter()
                .collect(),
            types: vec!["text/*".into(), "application/json".into()],
            min_bytes: 1024,
        }),
        ..HeaderPolicy::default()
    });
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire;
    unknown.header_policies[0]
        .compression
        .as_mut()
        .unwrap()
        .algorithms
        .push(42);
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("does not know"), "{refused}");
}

#[test]
fn upstream_resilience_round_trips_and_unknown_conditions_are_refused() {
    use panel_ir::{CircuitBreaker, RetryBudget, RetryCondition, UpstreamQueue};

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(12));
    let mut pool = UpstreamPoolSpec::new(UpstreamPoolId::new("grpc").unwrap(), "grpc", Vec::new());
    pool.retry_policy = RetryPolicy {
        attempts: 2,
        retry_statuses: [503].into_iter().collect(),
        retry_on: [RetryCondition::Timeout, RetryCondition::Reset].into(),
        backoff_ms: 25,
        budget: Some(RetryBudget {
            percent: 20,
            min_per_second: 3,
        }),
        ..RetryPolicy::none()
    };
    pool.circuit_breaker = Some(CircuitBreaker {
        failure_percent: 50,
        min_requests: 20,
        open_ms: 30_000,
        half_open_requests: 2,
    });
    pool.max_requests = Some(64);
    pool.queue = Some(UpstreamQueue {
        max_waiting: 100,
        timeout_ms: 2_000,
    });
    pool.connection.h2c = true;
    snapshot.upstream_pools.push(pool);
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire;
    unknown.upstream_pools[0]
        .retry_policy_v1
        .as_mut()
        .unwrap()
        .retry_on
        .push(42);
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("timeout or reset"), "{refused}");
}

#[test]
fn lua_programs_and_handlers_round_trip_and_unknown_kinds_are_refused() {
    use panel_ir::{
        LuaFallback, LuaHandler, LuaLogLevel, LuaPermissions, LuaProgram, LuaScript, LuaSharedDict,
        LuaVariable,
    };
    let script = |id: &str, module: Option<&str>| LuaScript {
        id: id.into(),
        file: id.split(':').next().unwrap_or(id).into(),
        line: 3,
        source: "ngx.say('hi')".into(),
        sha256: "00".repeat(32),
        module: module.map(Into::into),
    };
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(11));
    snapshot.lua = LuaProgram {
        disabled: false,
        scripts: vec![
            script("main.conf:3", None),
            script("lua/auth.lua", Some("auth")),
        ],
        init: Some(LuaHandler::new("lua/auth.lua")),
        init_worker: None,
        exit_worker: Some(LuaHandler::new("main.conf:3")),
        ssl_session_fetch: Some(LuaHandler::new("lua/auth.lua")),
        ssl_session_store: Some(LuaHandler::new("main.conf:3")),
        shared_dicts: vec![LuaSharedDict {
            name: "hits".into(),
            capacity_bytes: 1 << 20,
        }],
        memory_limit_bytes: 32 << 20,
        max_pending_timers: 64,
        max_running_timers: 8,
        regex_cache_max_entries: Some(0),
        regex_match_limit: 100_000,
        access_first: true,
        worker_thread_vm_pool_size: 4,
        capture_error_log_bytes: 32 << 10,
    };
    let site_id = SiteId::new("site").unwrap();
    let mut site = SiteSpec::new(
        site_id.clone(),
        "shop",
        vec![DomainSpec::new(
            NormalizedHost::new("shop.example").unwrap(),
        )],
    );
    site.lua.server_rewrite = Some(LuaHandler::new("main.conf:3"));
    site.lua.variables = vec![
        LuaVariable {
            name: "tenant".into(),
            value: "none".into(),
            handler: None,
            args: Vec::new(),
        },
        LuaVariable {
            name: "hash".into(),
            value: String::new(),
            handler: Some(LuaHandler::new("lua/auth.lua")),
            args: vec!["$http_x_key".into(), "${lua:tenant}".into()],
        },
    ];
    snapshot.sites.push(site);
    let mut access = LuaHandler::new("main.conf:3");
    access.time_limit_ms = 25;
    access.work_limit = 1_000;
    access.sockets.read_timeout_ms = 5_000;
    access.sockets.pool_size = 10;
    access.sockets.quiet = true;
    access.sockets.tls = Box::new(panel_ir::LuaTls {
        trusted_certificate_secret_id: Some("ca".into()),
        crl_secret_id: Some("crl".into()),
        certificate_secret_id: Some("client".into()),
        certificate_key_secret_id: Some("client-key".into()),
        verify_depth: Some(0),
        protocols: vec!["TLSv1.3".into()],
        cipher_suites: vec!["TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256".into()],
    });
    access.keep_underscores = true;
    access.no_default_type = true;
    access.read_body_first = true;
    access.check_client_abort = true;
    access.allow = LuaPermissions {
        body: true,
        upstream: false,
        network: true,
    };
    access.on_error = LuaFallback::Status { status: 503 };
    access.log_level = LuaLogLevel::Debug;
    access.slow_threshold_ms = 5;
    access.debug = true;
    let mut content = LuaHandler::new("main.conf:3");
    content.on_error = LuaFallback::Continue;
    let mut route = RouteSpec::new(
        RouteId::new("route").unwrap(),
        site_id,
        10,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/").unwrap(),
        },
        RouteAction::Lua { handler: content },
    );
    route.lua.access = Some(access);
    route.lua.log = Some(LuaHandler::new("lua/auth.lua"));
    snapshot.routes.push(route);
    let mut pool = UpstreamPoolSpec::new(
        UpstreamPoolId::new("pool").unwrap(),
        "app",
        vec![UpstreamEndpoint::new(
            EndpointId::new("node").unwrap(),
            EndpointAddress::new("127.0.0.1", 8080, false).unwrap(),
        )],
    );
    pool.balancer = Some(LuaHandler::new("lua/auth.lua"));
    snapshot.upstream_pools.push(pool);
    snapshot.refresh_content_hash();
    let wire = encode_snapshot(&snapshot);
    assert_eq!(decode_snapshot(wire.clone()).unwrap(), snapshot);

    let mut unknown = wire.clone();
    unknown.routes[0]
        .lua
        .as_mut()
        .unwrap()
        .access
        .as_mut()
        .unwrap()
        .on_error = Some(panel_contracts::gateway::v1::LuaFallback {
        kind: 42,
        status: 0,
    });
    let refused = decode_snapshot(unknown).unwrap_err();
    assert!(refused.message.contains("Lua fallback"), "{refused}");

    let mut nameless = wire;
    nameless.upstream_pools[0]
        .balancer
        .as_mut()
        .unwrap()
        .script_id = String::new();
    assert!(decode_snapshot(nameless).is_err());
}

#[test]
fn snapshots_without_lua_keep_their_canonical_hash() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(7));
    let canonical = String::from_utf8(snapshot.canonical_bytes()).unwrap();
    assert!(!canonical.contains("\"lua\""), "{canonical}");
}
