use super::*;
use panel_domain::{RevisionId, UpstreamPoolId};
use panel_ir::{DomainSpec, ListenerRef, RouteAction, RouteCondition, SiteSpec, ValueTest};

fn site(id: &str, hosts: &[&str]) -> SiteSpec {
    SiteSpec::new(
        SiteId::new(id).unwrap(),
        id,
        hosts
            .iter()
            .map(|host| DomainSpec::new(NormalizedHost::new(host).unwrap()))
            .collect(),
    )
}

fn route(id: &str, priority: u32, matcher: RouteMatcher) -> RouteSpec {
    RouteSpec::new(
        RouteId::new(id).unwrap(),
        SiteId::new("site").unwrap(),
        priority,
        matcher,
        RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("pool").unwrap(),
        },
    )
}

fn prefix(path: &str) -> RouteMatcher {
    RouteMatcher::PathPrefix {
        path: PathPrefix::new(path).unwrap(),
    }
}

fn request(host: &str, path: &str) -> SimulatedRequest {
    SimulatedRequest {
        method: "GET".into(),
        host: host.into(),
        path: path.into(),
        ..SimulatedRequest::default()
    }
}

fn selected(router: &Router, request: &SimulatedRequest) -> Option<String> {
    let site = router.site(router.lookup(&request.host)?.site);
    site.select(request)
        .map(|index| site.route(index).id.as_str().to_owned())
}

#[test]
fn priority_then_specificity_decide() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.sites.push(site("site", &["example.com"]));
    let host = NormalizedHost::new("example.com").unwrap();
    snapshot.routes = vec![
        route("generic", 10, prefix("/")),
        route(
            "host-default",
            10,
            RouteMatcher::Host { host: host.clone() },
        ),
        route("docs", 10, prefix("/docs")),
        route(
            "specific",
            10,
            RouteMatcher::HostPathPrefix {
                host,
                path: PathPrefix::new("/api").unwrap(),
            },
        ),
        route("priority", 1, prefix("/admin")),
        route(
            "exact",
            10,
            RouteMatcher::ExactPath {
                path: "/docs/index".into(),
            },
        ),
        route(
            "glob",
            10,
            RouteMatcher::Glob {
                pattern: "/assets/*.css".into(),
            },
        ),
        route(
            "regex",
            10,
            RouteMatcher::Regex {
                pattern: "^/v[0-9]+/".into(),
            },
        ),
    ];
    let router = Router::compile(&snapshot).unwrap();
    let select = |path| selected(&router, &request("example.com", path)).unwrap();
    assert_eq!(select("/api/v1"), "specific");
    assert_eq!(select("/admin"), "priority");
    assert_eq!(select("/docs/page"), "docs");
    assert_eq!(select("/docs/index"), "exact");
    assert_eq!(select("/assets/site.css"), "glob");
    assert_eq!(select("/assets/nested/site.css"), "host-default");
    assert_eq!(select("/v2/items"), "regex");
    assert_eq!(select("/other"), "host-default");
}

#[test]
fn prefixes_respect_segment_boundaries() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.sites.push(site("site", &["example.com"]));
    snapshot.routes.push(route("api", 1, prefix("/api/")));
    let router = Router::compile(&snapshot).unwrap();
    assert!(selected(&router, &request("example.com", "/api")).is_some());
    assert!(selected(&router, &request("example.com", "/api/users")).is_some());
    assert!(selected(&router, &request("example.com", "/apiculture")).is_none());
}

#[test]
fn wildcard_domains_cover_one_label_and_exact_names_win() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.sites = vec![
        site("wild", &["*.example.com"]),
        site("api", &["api.example.com"]),
    ];
    let router = Router::compile(&snapshot).unwrap();
    let site_of = |host| {
        router
            .lookup(host)
            .map(|entry| router.site(entry.site).id.as_str().to_owned())
    };
    assert_eq!(site_of("www.example.com").as_deref(), Some("wild"));
    assert_eq!(site_of("api.example.com").as_deref(), Some("api"));
    assert_eq!(site_of("a.b.example.com"), None);
    assert_eq!(site_of("example.com"), None);
}

#[test]
fn disabled_sites_domains_and_routes_are_omitted() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut primary = site("site", &["example.com", "old.example.com"]);
    primary.domains[1].enabled = false;
    snapshot.sites.push(primary);
    snapshot.routes.push(route("all", 1, prefix("/")));
    let router = Router::compile(&snapshot).unwrap();
    assert!(router.lookup("old.example.com").is_none());
    assert!(selected(&router, &request("example.com", "/")).is_some());
    snapshot.routes[0].enabled = false;
    let router = Router::compile(&snapshot).unwrap();
    assert!(selected(&router, &request("example.com", "/")).is_none());
    snapshot.sites[0].enabled = false;
    assert!(Router::compile(&snapshot)
        .unwrap()
        .lookup("example.com")
        .is_none());
}

#[test]
fn listeners_scope_sites_and_name_default_sites() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut http = ListenerRef::new("http", "0.0.0.0:80");
    http.default_site_id = Some(SiteId::new("site").unwrap());
    snapshot.listeners = vec![http, ListenerRef::new("https", "0.0.0.0:8443")];
    snapshot.sites.push(site("site", &["example.com"]));
    let mut internal = site("internal", &["internal.example"]);
    internal.listener_ids.insert("https".into());
    snapshot.sites.push(internal);
    let router = Router::compile(&snapshot).unwrap();
    assert_eq!(router.default_site("http"), Some(0));
    assert_eq!(router.default_site("https"), None);
    let internal = router.site(router.lookup("internal.example").unwrap().site);
    assert!(internal.serves("https") && !internal.serves("http"));
    assert_eq!(internal.spec, 1);
}

#[test]
fn invalid_patterns_fail_compilation() {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.sites.push(site("site", &["example.com"]));
    snapshot.routes.push(route(
        "regex",
        1,
        RouteMatcher::Regex {
            pattern: "(".into(),
        },
    ));
    assert!(Router::compile(&snapshot).is_err());
    snapshot.routes[0].matcher = RouteMatcher::Glob {
        pattern: "/[".into(),
    };
    assert!(Router::compile(&snapshot).is_err());
    snapshot.routes[0].matcher = RouteMatcher::Regex {
        pattern: "a{1000}{1000}".into(),
    };
    assert!(Router::compile(&snapshot).is_err());
}

fn equals(value: &str) -> ValueTest {
    ValueTest::Equals {
        value: value.into(),
        ignore_case: false,
    }
}

/// A site whose `/api` route takes only requests meeting `conditions`,
/// falling back to a route without conditions.
fn conditioned(conditions: Vec<RouteCondition>) -> Router {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .sites
        .push(site("site", &["shop.example", "*.shop.example"]));
    let mut guarded = route("guarded", 1, prefix("/api"));
    guarded.conditions = conditions;
    snapshot.routes = vec![guarded, route("fallback", 10, prefix("/"))];
    Router::compile(&snapshot).unwrap()
}

fn taken(router: &Router, request: &SimulatedRequest) -> String {
    selected(router, request).unwrap()
}

fn api() -> SimulatedRequest {
    request("www.shop.example", "/api/items")
}

#[test]
fn methods_and_hosts_are_compared_as_rfc_9110_says() {
    let router = conditioned(vec![
        RouteCondition::Method {
            methods: vec!["GET".into(), "HEAD".into()],
        },
        RouteCondition::Host {
            hosts: vec![NormalizedHost::new("*.shop.example").unwrap()],
        },
    ]);
    assert_eq!(taken(&router, &api()), "guarded");
    let apex = SimulatedRequest {
        host: "shop.example".into(),
        ..api()
    };
    assert_eq!(taken(&router, &apex), "fallback");
    let post = SimulatedRequest {
        method: "POST".into(),
        ..api()
    };
    assert_eq!(taken(&router, &post), "fallback");
    let lowercase = SimulatedRequest {
        method: "get".into(),
        ..api()
    };
    assert_eq!(
        taken(&router, &lowercase),
        "fallback",
        "methods are case-sensitive"
    );
}

#[test]
fn headers_ignore_name_case_and_combine_their_lines() {
    let router = conditioned(vec![RouteCondition::Header {
        name: "X-Env".into(),
        test: ValueTest::Contains {
            value: "STAGING".into(),
            ignore_case: true,
        },
    }]);
    let mut request = api();
    request.headers = vec![
        ("x-env".into(), "prod".into()),
        ("X-ENV".into(), "staging".into()),
    ];
    assert_eq!(taken(&router, &request), "guarded");
    request.headers.pop();
    assert_eq!(taken(&router, &request), "fallback");

    let exact = conditioned(vec![RouteCondition::Header {
        name: "x-tenant".into(),
        test: equals("a, b"),
    }]);
    let mut request = api();
    request.headers = vec![
        ("x-tenant".into(), "a".into()),
        ("x-tenant".into(), "b".into()),
    ];
    assert_eq!(
        taken(&exact, &request),
        "guarded",
        "lines combine with \", \""
    );
}

#[test]
fn query_parameters_are_decoded_and_any_repeated_value_counts() {
    let router = conditioned(vec![RouteCondition::Query {
        name: "tag".into(),
        test: equals("new arrivals"),
    }]);
    let mut request = api();
    request.query = Some("tag=sale&tag=new+arrivals".into());
    assert_eq!(taken(&router, &request), "guarded");
    request.query = Some("tag=new%20arrivals".into());
    assert_eq!(taken(&router, &request), "guarded");
    request.query = Some("tags=new+arrivals".into());
    assert_eq!(taken(&router, &request), "fallback");

    let absent = conditioned(vec![RouteCondition::Query {
        name: "debug".into(),
        test: ValueTest::Absent,
    }]);
    let mut request = api();
    assert_eq!(taken(&absent, &request), "guarded");
    request.query = Some("debug".into());
    assert_eq!(
        taken(&absent, &request),
        "fallback",
        "a bare name is present"
    );
}

#[test]
fn cookies_come_from_every_cookie_field() {
    let router = conditioned(vec![RouteCondition::Cookie {
        name: "beta".into(),
        test: ValueTest::Regex {
            pattern: "^on$".into(),
            ignore_case: false,
        },
    }]);
    let mut request = api();
    request.headers = vec![
        ("cookie".into(), "session=abc; theme=dark".into()),
        ("cookie".into(), "beta=\"on\"".into()),
    ];
    assert_eq!(taken(&router, &request), "guarded");
    request.headers = vec![("cookie".into(), "Beta=on".into())];
    assert_eq!(
        taken(&router, &request),
        "fallback",
        "cookie names are case-sensitive"
    );
}

#[test]
fn clients_match_networks_and_mapped_addresses_count_as_ipv4() {
    let router = conditioned(vec![RouteCondition::Client {
        networks: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
    }]);
    let from = |client: &str| SimulatedRequest {
        client: Some(client.parse().unwrap()),
        ..api()
    };
    assert_eq!(taken(&router, &from("10.1.2.3")), "guarded");
    assert_eq!(taken(&router, &from("::ffff:10.1.2.3")), "guarded");
    assert_eq!(taken(&router, &from("2001:db8::1")), "guarded");
    assert_eq!(taken(&router, &from("192.0.2.1")), "fallback");
    assert_eq!(
        taken(&router, &api()),
        "fallback",
        "an unknown client is outside"
    );
}

#[test]
fn user_agents_referers_and_content_types_are_tested() {
    let router = conditioned(vec![
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
    ]);
    let mut request = api();
    request.headers = vec![
        ("user-agent".into(), "ExampleBot/2.1".into()),
        ("referer".into(), "https://shop.example/cart".into()),
        (
            "content-type".into(),
            "Application/JSON; charset=utf-8".into(),
        ),
    ];
    assert_eq!(taken(&router, &request), "guarded");
    request.headers[2].1 = "text/plain".into();
    assert_eq!(taken(&router, &request), "guarded");
    request.headers[2].1 = "image/png".into();
    assert_eq!(taken(&router, &request), "fallback");
}

#[test]
fn groups_and_negation_combine_conditions() {
    let canary = RouteCondition::Any {
        conditions: vec![
            RouteCondition::Header {
                name: "x-canary".into(),
                test: equals("1"),
            },
            RouteCondition::Cookie {
                name: "canary".into(),
                test: equals("1"),
            },
        ],
    };
    let router = conditioned(vec![
        canary,
        RouteCondition::Not {
            condition: Box::new(RouteCondition::Client {
                networks: vec!["192.0.2.0/24".into()],
            }),
        },
    ]);
    let mut request = api();
    request.client = Some("203.0.113.9".parse().unwrap());
    request.headers = vec![("cookie".into(), "canary=1".into())];
    assert_eq!(taken(&router, &request), "guarded");
    request.client = Some("192.0.2.7".parse().unwrap());
    assert_eq!(taken(&router, &request), "fallback");
    request.client = Some("203.0.113.9".parse().unwrap());
    request.headers.clear();
    assert_eq!(taken(&router, &request), "fallback");
}

#[test]
fn mismatches_name_the_first_part_that_does_not_hold() {
    let router = conditioned(vec![
        RouteCondition::Method {
            methods: vec!["POST".into()],
        },
        RouteCondition::Header {
            name: "x-env".into(),
            test: equals("staging"),
        },
    ]);
    let site = router.site(0);
    let guarded = site.route(0);
    let mut request = api();
    assert_eq!(
        guarded.mismatch(&request),
        Some(Mismatch::Condition(
            "method POST (the method is GET)".into()
        ))
    );
    request.method = "POST".into();
    request.headers = vec![("x-env".into(), "prod".into())];
    assert_eq!(
        guarded.mismatch(&request).unwrap().to_string(),
        "header x-env = \"staging\" (it is \"prod\") does not hold"
    );
    request.path = "/home".into();
    assert_eq!(
        guarded.mismatch(&request),
        Some(Mismatch::Path("prefix /api".into()))
    );
    request.path = "/api".into();
    request.headers = vec![("x-env".into(), "staging".into())];
    assert_eq!(guarded.mismatch(&request), None);
}

#[test]
fn compiled_conditions_hold_when_any_of_them_does() {
    use panel_ir::{RouteCondition, ValueTest};

    let conditions = crate::CompiledConditions::compile(&[
        RouteCondition::Cookie {
            name: "session".into(),
            test: ValueTest::Present,
        },
        RouteCondition::Query {
            name: "nocache".into(),
            test: ValueTest::Present,
        },
    ])
    .unwrap();
    let request = |query: Option<&str>, headers: &[(&str, &str)]| crate::SimulatedRequest {
        method: "GET".into(),
        host: "shop.example".into(),
        path: "/".into(),
        query: query.map(str::to_owned),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        client: None,
    };
    assert!(!conditions.any_holds(&request(None, &[])));
    assert!(conditions.any_holds(&request(Some("nocache=1"), &[])));
    assert!(conditions.any_holds(&request(None, &[("cookie", "a=1; session=x")])));
    assert!(!crate::CompiledConditions::compile(&[]).unwrap().any_holds(&request(None, &[])));
}
