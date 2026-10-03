//! Routes that are valid but can never match: shadowed by a route the
//! gateway evaluates first, or restricted to a host the server does not
//! answer for.

use crate::codes;
use globset::GlobBuilder;
use panel_config_model::{ConfigModel, MatchKind, Route};
use panel_errors::Diagnostic;
use std::cmp::Reverse;
use uuid::Uuid;

/// The gateway's evaluation order: priority, then the more specific path
/// matcher, then a concrete host, then the identifier.
fn rank(route: &Route) -> (u32, Reverse<(u8, usize)>, Reverse<u8>, String) {
    let path = &route.matcher.path;
    let specificity = match route.matcher.kind {
        MatchKind::Exact => (4, path.len()),
        MatchKind::Glob => (3, path.len()),
        MatchKind::Regex => (2, path.len()),
        MatchKind::Prefix => match prefix(path) {
            "" => (0, 0),
            prefix => (1, prefix.len()),
        },
        _ => (0, 0),
    };
    let host = route
        .matcher
        .host
        .as_ref()
        .map_or(0, |host| if host.is_wildcard() { 1 } else { 2 });
    (
        route.priority,
        Reverse(specificity),
        Reverse(host),
        route.id.to_string(),
    )
}

/// A prefix as the gateway compares it, without trailing slashes.
fn prefix(path: &str) -> &str {
    path.trim_end_matches('/')
}

fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `first` takes every request `later` could match.
fn covers(first: &Route, later: &Route) -> bool {
    if first.matcher.host.is_some() && first.matcher.host != later.matcher.host {
        return false;
    }
    let (a, b) = (&first.matcher, &later.matcher);
    match (a.kind, b.kind) {
        (MatchKind::Prefix, MatchKind::Prefix) => under(prefix(&b.path), prefix(&a.path)),
        (MatchKind::Prefix, MatchKind::Exact) => under(&b.path, prefix(&a.path)),
        (MatchKind::Prefix, _) => prefix(&a.path).is_empty(),
        (MatchKind::Exact, MatchKind::Exact) => a.path == b.path,
        (MatchKind::Glob, MatchKind::Glob) | (MatchKind::Regex, MatchKind::Regex) => {
            a.path == b.path
        }
        (MatchKind::Glob, MatchKind::Exact) => GlobBuilder::new(&a.path)
            .literal_separator(true)
            .build()
            .is_ok_and(|glob| glob.compile_matcher().is_match(&b.path)),
        (MatchKind::Regex, MatchKind::Exact) => {
            regex::Regex::new(&a.path).is_ok_and(|regex| regex.is_match(&b.path))
        }
        _ => false,
    }
}

pub(crate) fn label(route: &Route) -> String {
    route.name.as_ref().map_or_else(
        || format!("{:?}", route.matcher.path),
        |name| format!("{name:?}"),
    )
}

/// Reports, by site and route, every enabled route that cannot match.
pub(crate) fn routes(model: &ConfigModel, report: &mut dyn FnMut(Uuid, Uuid, Diagnostic)) {
    for site in model.sites.iter().filter(|site| !site.is_deleted()) {
        let mut ordered: Vec<&Route> = site.routes.iter().filter(|route| route.enabled).collect();
        ordered.sort_by_cached_key(|route| rank(route));
        for (index, route) in ordered.iter().enumerate() {
            if let Some(host) = &route.matcher.host {
                let served = site
                    .domains
                    .iter()
                    .any(|domain| domain.enabled && !domain.redirect && domain.host == *host);
                if !served {
                    let diagnostic = Diagnostic::warning(
                        codes::UNREACHABLE_ROUTE,
                        format!(
                            "route {} only matches {host}, which the server does not serve",
                            label(route)
                        ),
                    )
                    .with_help("add the host to the server or remove the host restriction");
                    report(site.id, route.id, diagnostic);
                    continue;
                }
            }
            if let Some(first) = ordered[..index].iter().find(|first| covers(first, route)) {
                let diagnostic = Diagnostic::warning(
                    codes::SHADOWED_ROUTE,
                    format!(
                        "route {} never matches: route {} is evaluated first and takes all of its requests",
                        label(route),
                        label(first)
                    ),
                )
                .with_help("give the narrower route a lower priority, or remove one of them");
                report(site.id, route.id, diagnostic);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_config_model::{Action, Domain, RouteMatch, Site};
    use panel_domain::NormalizedHost;

    fn route(name: &str, priority: u32, kind: MatchKind, path: &str, host: Option<&str>) -> Route {
        Route {
            id: Uuid::now_v7(),
            name: Some(name.into()),
            enabled: true,
            priority,
            matcher: RouteMatch {
                kind,
                path: path.into(),
                host: host.map(|host| NormalizedHost::new(host).unwrap()),
            },
            action: Action::Respond {
                status: 204,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
            security_policy_id: Default::default(),
        }
    }

    fn findings(routes: Vec<Route>) -> Vec<(String, String)> {
        let now = chrono::Utc::now();
        let site = Site {
            id: Uuid::now_v7(),
            name: "shop".into(),
            action: Action::Respond {
                status: 204,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
            enabled: true,
            domains: vec![Domain {
                host: NormalizedHost::new("shop.example").unwrap(),
                enabled: true,
                primary: true,
                redirect: false,
                tls_profile_id: None,
            }],
            routes,
            listener_ids: Default::default(),
            https_redirect: false,
            www_redirect: Default::default(),
            tls_profile_id: None,
            hsts: None,
            group: None,
            tags: Default::default(),
            note: None,
            favorite: false,
            deleted_at: None,
            created_at: now,
            updated_at: now,
            security_policy_id: Default::default(),
        };
        let model = ConfigModel {
            sites: vec![site],
            ..ConfigModel::default()
        };
        let mut found = Vec::new();
        routes_of(&model, &mut found);
        found
    }

    fn routes_of(model: &ConfigModel, found: &mut Vec<(String, String)>) {
        routes(model, &mut |_, route, diagnostic| {
            let name = model.sites[0]
                .routes
                .iter()
                .find(|r| r.id == route)
                .unwrap()
                .name
                .clone()
                .unwrap();
            found.push((name, diagnostic.code.as_str().to_owned()));
        });
    }

    #[test]
    fn earlier_routes_that_take_every_request_shadow_later_ones() {
        let found = findings(vec![
            route("api", 10, MatchKind::Prefix, "/api/", None),
            route("v1", 20, MatchKind::Prefix, "/api/v1", None),
            route("health", 30, MatchKind::Exact, "/api/health", None),
            route("apiary", 40, MatchKind::Prefix, "/apiary", None),
            route("glob", 50, MatchKind::Glob, "/static/*.css", None),
            route("css", 60, MatchKind::Exact, "/static/site.css", None),
            route("deep", 70, MatchKind::Exact, "/static/a/site.css", None),
        ]);
        assert_eq!(
            found,
            vec![
                ("v1".into(), codes::SHADOWED_ROUTE.into()),
                ("health".into(), codes::SHADOWED_ROUTE.into()),
                ("css".into(), codes::SHADOWED_ROUTE.into()),
            ]
        );
    }

    #[test]
    fn specificity_orders_routes_of_equal_priority() {
        let found = findings(vec![
            route("any", 10, MatchKind::Prefix, "/", None),
            route("exact", 10, MatchKind::Exact, "/x", None),
            route("later", 20, MatchKind::Exact, "/y", None),
        ]);
        assert_eq!(found, vec![("later".into(), codes::SHADOWED_ROUTE.into())]);
    }

    #[test]
    fn host_restrictions_must_name_a_served_host() {
        let found = findings(vec![
            route("other", 10, MatchKind::Prefix, "/", Some("other.example")),
            route("own", 20, MatchKind::Prefix, "/a", Some("shop.example")),
            route("after", 30, MatchKind::Prefix, "/a/b", None),
        ]);
        assert_eq!(
            found,
            vec![("other".into(), codes::UNREACHABLE_ROUTE.into())]
        );
    }
}
