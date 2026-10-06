//! Engine-neutral checks for listeners, site redirects, route actions and
//! upstream policies.

use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    template::{parse_template, uses_variables, TEMPLATE_CAPABILITY},
    ActiveHealthCheck, HealthCheckProtocol, ListenerRef, LoadBalancingPolicy, RouteAction,
    RouteMatcher, RuntimeSnapshot, UpstreamPoolSpec, ROUTE_CONDITIONS_CAPABILITY,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
};

/// The compiled size, and lazy DFA cache, allowed for one route regular
/// expression; engines compile route patterns with this limit.
pub const ROUTE_REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Why a route's regular expression would not compile, in one line.
pub fn route_regex_error(pattern: &str) -> Option<String> {
    regex::RegexBuilder::new(pattern)
        .size_limit(ROUTE_REGEX_SIZE_LIMIT)
        .dfa_size_limit(ROUTE_REGEX_SIZE_LIMIT)
        .build()
        .err()
        .map(|error| {
            let text = error.to_string();
            text.lines().last().map_or(text.clone(), |line| {
                line.trim_start_matches("error: ").to_owned()
            })
        })
}

const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];
const MIN_HEALTH_CHECK_INTERVAL_MS: u64 = 100;

pub(crate) fn validate_traffic(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let templates_declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == TEMPLATE_CAPABILITY);
    let conditions_declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == ROUTE_CONDITIONS_CAPABILITY);
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    let site_ids: BTreeSet<_> = snapshot.sites.iter().map(|site| &site.id).collect();
    let tls_profiles: BTreeMap<_, _> = snapshot
        .tls_profiles
        .iter()
        .map(|profile| (profile.id.as_str(), profile))
        .collect();
    let static_policies: BTreeSet<_> = snapshot
        .static_content
        .iter()
        .map(|policy| policy.id.as_str())
        .collect();

    let mut listener_ids = BTreeSet::new();
    let mut bound: Vec<(&ListenerRef, SocketAddr)> = Vec::new();
    for listener in &snapshot.listeners {
        if listener.id.is_empty() || !listener_ids.insert(listener.id.as_str()) {
            report(
                &listener.id,
                format!("listener id {:?} is empty or duplicated", listener.id),
            );
        }
        match listener.address.parse::<SocketAddr>() {
            Ok(address) if address.port() == 0 => report(
                &listener.id,
                format!("listener {} must use a non-zero port", listener.id),
            ),
            Ok(address) => {
                if let Some((other, _)) = bound.iter().find(|(other, other_address)| {
                    sockets_conflict(listener, address, other, *other_address)
                }) {
                    report(
                        &listener.id,
                        format!(
                            "listener {} address {} conflicts with listener {} address {}",
                            listener.id, listener.address, other.id, other.address
                        ),
                    );
                }
                bound.push((listener, address));
            }
            Err(_) => report(
                &listener.id,
                format!(
                    "listener {} address {:?} is not an IP socket address",
                    listener.id, listener.address
                ),
            ),
        }
        if !listener.protocols.http1 && !listener.protocols.http2 {
            report(
                &listener.id,
                format!("listener {} must accept HTTP/1.1 or HTTP/2", listener.id),
            );
        }
        if let Some(id) = listener.tls_profile_id.as_deref() {
            match tls_profiles.get(id) {
                None => report(
                    &listener.id,
                    format!(
                        "listener {} references unknown TLS profile {id}",
                        listener.id
                    ),
                ),
                Some(profile) => {
                    let offers =
                        |protocol: &str| profile.alpn.is_empty() || profile.alpn.contains(protocol);
                    let enabled = listener.protocols.http1 || listener.protocols.http2;
                    if enabled
                        && !(listener.protocols.http1 && offers("http/1.1")
                            || listener.protocols.http2 && offers("h2"))
                    {
                        report(
                            &listener.id,
                            format!(
                                "TLS profile {id} offers none of listener {}'s protocols",
                                listener.id
                            ),
                        );
                    }
                }
            }
        }
        if let Some(site) = listener
            .default_site_id
            .as_ref()
            .filter(|site| !site_ids.contains(site))
        {
            report(
                &listener.id,
                format!(
                    "listener {} default site {site} does not exist",
                    listener.id
                ),
            );
        }
    }

    for site in &snapshot.sites {
        let resource = site.id.as_str();
        for listener in site
            .listener_ids
            .iter()
            .filter(|listener| !listener_ids.contains(listener.as_str()))
        {
            report(
                resource,
                format!("site {} references unknown listener {listener}", site.id),
            );
        }
        let serves_tls = snapshot
            .listeners
            .iter()
            .filter(|listener| {
                site.listener_ids.is_empty() || site.listener_ids.contains(&listener.id)
            })
            .any(|listener| listener.tls_profile_id.is_some());
        if site.https_redirect && !serves_tls {
            report(
                resource,
                format!(
                    "site {} redirects to HTTPS but no TLS listener serves it",
                    site.id
                ),
            );
        }
        let primaries: Vec<_> = site
            .domains
            .iter()
            .filter(|domain| domain.primary)
            .collect();
        if primaries.len() > 1 {
            report(
                resource,
                format!("site {} has more than one primary domain", site.id),
            );
        }
        if let Some(primary) = primaries.first() {
            if primary.host.is_wildcard() || !primary.enabled || primary.redirect_to_primary {
                report(
                    resource,
                    format!(
                        "site {} primary domain {} must be an enabled, concrete name",
                        site.id, primary.host
                    ),
                );
            }
        }
        if primaries.is_empty() && site.domains.iter().any(|domain| domain.redirect_to_primary) {
            report(
                resource,
                format!(
                    "site {} redirects aliases but has no primary domain",
                    site.id
                ),
            );
        }
        for domain in &site.domains {
            if let Some(profile) = domain
                .tls_profile_id
                .as_deref()
                .filter(|profile| !tls_profiles.contains_key(profile))
            {
                report(
                    resource,
                    format!(
                        "domain {} references unknown TLS profile {profile}",
                        domain.host
                    ),
                );
            }
        }
    }

    let mut route_names = BTreeSet::new();
    let mut named_locations = BTreeSet::new();
    for route in &snapshot.routes {
        let resource = route.id.as_str();
        if let Some(name) = &route.name {
            if name.trim().is_empty() || name.len() > 128 {
                report(
                    resource,
                    format!("route {} name must contain 1..=128 bytes", route.id),
                );
            } else if !route_names.insert((&route.site_id, name.as_str())) {
                report(
                    resource,
                    format!(
                        "route name {name:?} is used more than once in site {}",
                        route.site_id
                    ),
                );
            }
        }
        match &route.matcher {
            RouteMatcher::ExactPath { path } if !path.starts_with('/') => report(
                resource,
                format!("route {} exact path must start with '/'", route.id),
            ),
            RouteMatcher::Glob { pattern } if !pattern.starts_with('/') => report(
                resource,
                format!("route {} glob must start with '/'", route.id),
            ),
            RouteMatcher::Regex { pattern } if pattern.is_empty() || pattern.len() > 1024 => {
                report(
                    resource,
                    format!("route {} regex must contain 1..=1024 bytes", route.id),
                )
            }
            RouteMatcher::Regex { pattern } => {
                if let Some(error) = route_regex_error(pattern) {
                    report(
                        resource,
                        format!("route {} regex does not compile: {error}", route.id),
                    );
                }
            }
            RouteMatcher::Named { name } => {
                let valid = !name.is_empty()
                    && name.len() <= 128
                    && name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    });
                if !valid {
                    report(
                        resource,
                        format!("route {} named location {name:?} is not a name", route.id),
                    );
                } else if !named_locations.insert((&route.site_id, name.as_str())) {
                    report(
                        resource,
                        format!(
                            "named location @{name} is defined more than once in site {}",
                            route.site_id
                        ),
                    );
                }
                if !route.conditions.is_empty() {
                    report(
                        resource,
                        format!(
                            "route {} is a named location and matches no conditions",
                            route.id
                        ),
                    );
                }
            }
            _ => {}
        }
        for problem in crate::conditions::problems(&route.conditions) {
            report(resource, format!("route {} {problem}", route.id));
        }
        if !route.conditions.is_empty() && !conditions_declared {
            report(
                resource,
                format!(
                    "route {} has conditions without requiring {ROUTE_CONDITIONS_CAPABILITY}",
                    route.id
                ),
            );
        }
        let templates: Vec<&str> = match &route.action {
            RouteAction::Redirect { location, .. } => vec![location.as_str()],
            RouteAction::Respond {
                body: Some(body), ..
            } => vec![body.as_str()],
            _ => Vec::new(),
        };
        for template in templates {
            match parse_template(template) {
                Err(error) => report(
                    resource,
                    format!("route {} has an invalid template: {error}", route.id),
                ),
                Ok(_) if uses_variables(template) && !templates_declared => report(
                    resource,
                    format!(
                        "route {} uses request variables without requiring {TEMPLATE_CAPABILITY}",
                        route.id
                    ),
                ),
                Ok(_) => {}
            }
        }
        match &route.action {
            RouteAction::Redirect {
                location, status, ..
            } => {
                if !REDIRECT_STATUSES.contains(status) {
                    report(
                        resource,
                        format!(
                            "route {} redirect status {status} is not a redirection",
                            route.id
                        ),
                    );
                }
                if location.is_empty() || location.contains(char::is_control) {
                    report(
                        resource,
                        format!("route {} redirect location is invalid", route.id),
                    );
                }
            }
            RouteAction::Respond {
                content_type: Some(content_type),
                ..
            } if content_type.is_empty() || content_type.contains(char::is_control) => report(
                resource,
                format!("route {} response content type is invalid", route.id),
            ),
            RouteAction::Static { policy_id } if !static_policies.contains(policy_id.as_str()) => {
                report(
                    resource,
                    format!(
                        "route {} references unknown static content {policy_id}",
                        route.id
                    ),
                )
            }
            _ => {}
        }
    }

    for pool in &snapshot.upstream_pools {
        validate_pool(pool, &mut report);
    }
}

fn validate_pool(pool: &UpstreamPoolSpec, report: &mut impl FnMut(&str, String)) {
    let resource = pool.id.as_str();
    if let LoadBalancingPolicy::ConsistentHash { key } = &pool.load_balancing {
        let valid = match key.split_once(':') {
            Some(("header" | "cookie", name)) => is_token(name),
            Some(_) => false,
            None => matches!(key.as_str(), "client_ip" | "uri"),
        };
        if !valid {
            report(
                resource,
                format!(
                    "upstream {} hash key {key:?} must be client_ip, uri, header:<name> or cookie:<name>",
                    pool.id
                ),
            );
        }
    }
    let connection = &pool.connection;
    for (name, value) in [
        ("connect timeout", connection.connect_timeout_ms),
        ("read timeout", connection.read_timeout_ms),
        ("write timeout", connection.write_timeout_ms),
        ("idle timeout", connection.idle_timeout_ms),
    ] {
        if value == Some(0) {
            report(
                resource,
                format!("upstream {} {name} must be positive", pool.id),
            );
        }
    }
    if connection.max_connections == Some(0) {
        report(
            resource,
            format!("upstream {} max connections must be positive", pool.id),
        );
    }
    for (name, value) in [
        ("CA secret", pool.tls.ca_secret_id.as_deref()),
        ("SNI", pool.tls.sni.as_deref()),
        ("host header", pool.host_header.as_deref()),
    ] {
        if value.is_some_and(|value| value.is_empty() || value.contains(char::is_control)) {
            report(resource, format!("upstream {} {name} is invalid", pool.id));
        }
    }
    if let Some(check) = &pool.health_check {
        validate_health_check(pool, check, report);
    }
    if let Some(passive) = &pool.passive_health {
        if passive.failure_threshold == 0 || passive.ejection_ms == 0 {
            report(
                resource,
                format!(
                    "upstream {} passive health thresholds must be positive",
                    pool.id
                ),
            );
        }
    }
    for endpoint in &pool.endpoints {
        if endpoint
            .unix_socket
            .as_deref()
            .is_some_and(|path| !path.starts_with('/') || path.contains('\0'))
        {
            report(
                resource,
                format!(
                    "upstream endpoint {} socket must be an absolute path",
                    endpoint.id
                ),
            );
        }
    }
}

fn validate_health_check(
    pool: &UpstreamPoolSpec,
    check: &ActiveHealthCheck,
    report: &mut impl FnMut(&str, String),
) {
    let resource = pool.id.as_str();
    if check.interval_ms < MIN_HEALTH_CHECK_INTERVAL_MS
        || check.timeout_ms == 0
        || check.timeout_ms > check.interval_ms
    {
        report(
            resource,
            format!(
                "upstream {} health check needs an interval of at least {MIN_HEALTH_CHECK_INTERVAL_MS} ms and a timeout within it",
                pool.id
            ),
        );
    }
    if check.healthy_threshold == 0 || check.unhealthy_threshold == 0 {
        report(
            resource,
            format!(
                "upstream {} health check thresholds must be positive",
                pool.id
            ),
        );
    }
    if check.protocol == HealthCheckProtocol::Http {
        if !check.path.starts_with('/') || check.path.contains(char::is_whitespace) {
            report(
                resource,
                format!(
                    "upstream {} health check path must be an absolute path",
                    pool.id
                ),
            );
        }
        if !matches!(check.method.as_str(), "GET" | "HEAD") {
            report(
                resource,
                format!(
                    "upstream {} health check method must be GET or HEAD",
                    pool.id
                ),
            );
        }
        if check
            .expected_statuses
            .iter()
            .any(|status| !(100..=599).contains(status))
        {
            report(
                resource,
                format!(
                    "upstream {} health check expects an invalid status",
                    pool.id
                ),
            );
        }
    }
}

/// Two listeners conflict when the operating system would refuse to bind both;
/// a dual-stack IPv6 wildcard also covers every IPv4 address.
fn sockets_conflict(
    listener: &ListenerRef,
    address: SocketAddr,
    other: &ListenerRef,
    other_address: SocketAddr,
) -> bool {
    if address.port() != other_address.port() {
        return false;
    }
    match (address.ip(), other_address.ip()) {
        (IpAddr::V4(left), IpAddr::V4(right)) => {
            left == right || left.is_unspecified() || right.is_unspecified()
        }
        (IpAddr::V6(left), IpAddr::V6(right)) => {
            left == right || left.is_unspecified() || right.is_unspecified()
        }
        (IpAddr::V6(left), IpAddr::V4(_)) => {
            left.is_unspecified() && listener.ipv6_only != Some(true)
        }
        (IpAddr::V4(_), IpAddr::V6(right)) => {
            right.is_unspecified() && other.ipv6_only != Some(true)
        }
    }
}

/// RFC 9110 §5.6.2 token characters.
fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

#[cfg(test)]
mod tests {
    use crate::validate_engine_ir;
    use panel_domain::{
        EndpointAddress, EndpointId, NormalizedHost, RevisionId, RouteId, SiteId, UpstreamPoolId,
    };
    use panel_ir::{
        DomainSpec, ListenerRef, LoadBalancingPolicy, PassiveHealthPolicy, RouteAction,
        RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec, TlsProfile, UpstreamEndpoint,
        UpstreamPoolSpec,
    };
    use std::collections::BTreeSet;

    fn messages(snapshot: &mut RuntimeSnapshot) -> Vec<String> {
        snapshot.refresh_content_hash();
        validate_engine_ir(snapshot, &BTreeSet::new())
            .unwrap()
            .diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn site() -> SiteSpec {
        SiteSpec::new(
            SiteId::new("site").unwrap(),
            "site",
            vec![DomainSpec::new(NormalizedHost::new("example.com").unwrap())],
        )
    }

    #[test]
    fn listener_conflicts_follow_socket_binding_rules() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.listeners = vec![
            ListenerRef::new("any", "0.0.0.0:80"),
            ListenerRef::new("loopback", "127.0.0.1:80"),
            ListenerRef::new("dual", "[::]:80"),
            ListenerRef::new("other-port", "127.0.0.1:8080"),
        ];
        let found = messages(&mut snapshot);
        assert!(found
            .iter()
            .any(|message| message
                .contains("loopback address 127.0.0.1:80 conflicts with listener any")));
        assert!(found
            .iter()
            .any(|message| message.contains("dual address [::]:80 conflicts")));
        assert!(!found.iter().any(|message| message.contains("other-port")));

        snapshot.listeners = vec![ListenerRef::new("v4", "0.0.0.0:80"), {
            let mut listener = ListenerRef::new("v6", "[::]:80");
            listener.ipv6_only = Some(true);
            listener
        }];
        assert!(messages(&mut snapshot).is_empty());
    }

    #[test]
    fn listener_references_and_protocols_are_checked() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut listener = ListenerRef::new("https", "not-an-address");
        listener.tls_profile_id = Some("missing".into());
        listener.default_site_id = Some(SiteId::new("ghost").unwrap());
        listener.protocols.http1 = false;
        listener.protocols.http2 = false;
        snapshot.listeners.push(listener);
        let found = messages(&mut snapshot);
        for expected in [
            "is not an IP socket address",
            "unknown TLS profile missing",
            "default site ghost does not exist",
            "must accept HTTP/1.1 or HTTP/2",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn https_redirect_requires_a_tls_listener_and_aliases_need_a_primary() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot
            .listeners
            .push(ListenerRef::new("http", "0.0.0.0:80"));
        let mut site = site();
        site.https_redirect = true;
        let mut alias = DomainSpec::new(NormalizedHost::new("example.net").unwrap());
        alias.redirect_to_primary = true;
        site.domains.push(alias);
        snapshot.sites.push(site);
        let found = messages(&mut snapshot);
        assert!(found
            .iter()
            .any(|message| message.contains("no TLS listener serves it")));
        assert!(found
            .iter()
            .any(|message| message.contains("has no primary domain")));

        snapshot.tls_profiles.push(TlsProfile {
            id: "tls".into(),
            certificate_secret_id: "cert".into(),
            private_key_secret_id: "key".into(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            alpn: BTreeSet::new(),
        });
        let mut https = ListenerRef::new("https", "0.0.0.0:443");
        https.tls_profile_id = Some("tls".into());
        snapshot.listeners.push(https);
        snapshot.sites[0].domains[0].primary = true;
        assert!(messages(&mut snapshot).is_empty());

        snapshot.tls_profiles[0].alpn = ["h2".to_owned()].into();
        snapshot.listeners[1].protocols.http2 = false;
        let found = messages(&mut snapshot);
        assert!(
            found.iter().any(|message| message
                .contains("TLS profile tls offers none of listener https's protocols")),
            "{found:?}"
        );
        snapshot.listeners[1].protocols.http2 = true;
        assert!(messages(&mut snapshot).is_empty());
    }

    #[test]
    fn route_patterns_compile_as_engines_compile_them() {
        assert_eq!(crate::route_regex_error(r"(?i)\.(png|jpg)$"), None);
        assert_eq!(crate::route_regex_error(r"^/user/(?<id>\d+)$"), None);
        assert_eq!(
            crate::route_regex_error("^(/api").as_deref(),
            Some("unclosed group")
        );
    }

    #[test]
    fn route_actions_and_names_are_checked() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site());
        let route = |id: &str, matcher, action| {
            let mut route = RouteSpec::new(
                RouteId::new(id).unwrap(),
                SiteId::new("site").unwrap(),
                1,
                matcher,
                action,
            );
            route.name = Some("same".into());
            route
        };
        snapshot.routes = vec![
            route(
                "redirect",
                RouteMatcher::ExactPath { path: "old".into() },
                RouteAction::redirect("https://example.com/", 200),
            ),
            route(
                "static",
                RouteMatcher::Glob {
                    pattern: "*.css".into(),
                },
                RouteAction::Static {
                    policy_id: "missing".into(),
                },
            ),
            route(
                "pattern",
                RouteMatcher::Regex {
                    pattern: "^(/api".into(),
                },
                RouteAction::redirect("https://example.com/", 301),
            ),
        ];
        let found = messages(&mut snapshot);
        for expected in [
            "route pattern regex does not compile: unclosed group",
            "exact path must start with '/'",
            "redirect status 200 is not a redirection",
            "glob must start with '/'",
            "unknown static content missing",
            "route name \"same\" is used more than once",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn upstream_policies_are_bounded() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut endpoint = UpstreamEndpoint::new(
            EndpointId::new("node").unwrap(),
            EndpointAddress::new("127.0.0.1", 8080, false).unwrap(),
        );
        endpoint.unix_socket = Some("relative.sock".into());
        let mut pool =
            UpstreamPoolSpec::new(UpstreamPoolId::new("pool").unwrap(), "pool", vec![endpoint]);
        pool.load_balancing = LoadBalancingPolicy::ConsistentHash {
            key: "header:bad name".into(),
        };
        pool.connection.connect_timeout_ms = Some(0);
        pool.connection.max_connections = Some(0);
        pool.host_header = Some("bad\r\nhost".into());
        pool.passive_health = Some(PassiveHealthPolicy {
            failure_threshold: 0,
            ejection_ms: 1,
        });
        pool.health_check = Some(panel_ir::ActiveHealthCheck {
            protocol: panel_ir::HealthCheckProtocol::Http,
            path: "health".into(),
            method: "POST".into(),
            interval_ms: 50,
            timeout_ms: 100,
            healthy_threshold: 0,
            unhealthy_threshold: 1,
            expected_statuses: BTreeSet::from([700]),
            host: None,
        });
        snapshot.upstream_pools.push(pool);
        let found = messages(&mut snapshot);
        for expected in [
            "hash key \"header:bad name\"",
            "connect timeout must be positive",
            "max connections must be positive",
            "host header is invalid",
            "passive health thresholds must be positive",
            "interval of at least 100 ms",
            "thresholds must be positive",
            "path must be an absolute path",
            "method must be GET or HEAD",
            "expects an invalid status",
            "socket must be an absolute path",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
        snapshot.upstream_pools[0].load_balancing = LoadBalancingPolicy::ConsistentHash {
            key: "cookie:session".into(),
        };
        assert!(!messages(&mut snapshot)
            .iter()
            .any(|message| message.contains("hash key")));
    }
}
