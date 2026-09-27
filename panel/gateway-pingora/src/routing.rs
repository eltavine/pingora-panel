//! Immutable routing decisions compiled before a snapshot becomes active.

use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{RouteAction, RouteMatcher, RuntimeSnapshot};
use std::{cmp::Reverse, collections::BTreeMap};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyRouteSelection {
    route_id: RouteId,
    upstream_pool_id: UpstreamPoolId,
}

impl ProxyRouteSelection {
    pub fn route_id(&self) -> &RouteId {
        &self.route_id
    }

    pub fn upstream_pool_id(&self) -> &UpstreamPoolId {
        &self.upstream_pool_id
    }
}

pub(crate) struct RouteIndex {
    by_host: BTreeMap<NormalizedHost, Vec<CompiledRoute>>,
}

struct CompiledRoute {
    priority: u32,
    route_id: RouteId,
    upstream_pool_id: UpstreamPoolId,
    path_prefix: Option<PathPrefix>,
    specificity: (usize, u8),
}

impl RouteIndex {
    pub(crate) fn compile(snapshot: &RuntimeSnapshot) -> Result<Self> {
        let enabled_sites: BTreeMap<SiteId, Vec<NormalizedHost>> = snapshot
            .sites
            .iter()
            .filter(|site| site.enabled)
            .map(|site| {
                (
                    site.id.clone(),
                    site.domains
                        .iter()
                        .map(|domain| domain.host.clone())
                        .collect(),
                )
            })
            .collect();
        let mut by_host: BTreeMap<NormalizedHost, Vec<CompiledRoute>> = BTreeMap::new();
        for route in snapshot.routes.iter().filter(|route| route.enabled) {
            let Some(domains) = enabled_sites.get(&route.site_id) else {
                continue;
            };
            let (matcher_host, path_prefix) = match &route.matcher {
                RouteMatcher::Host { host } => (Some(host), None),
                RouteMatcher::PathPrefix { path } => (None, Some(path.clone())),
                RouteMatcher::HostPathPrefix { host, path } => (Some(host), Some(path.clone())),
                _ => {
                    return Err(PanelError::unsupported_capability(format!(
                        "route {} uses a matcher unsupported by the Pingora route compiler",
                        route.id
                    )))
                }
            };
            let RouteAction::Proxy { upstream_pool_id } = &route.action else {
                return Err(PanelError::unsupported_capability(format!(
                    "route {} uses an action unsupported by the Pingora route compiler",
                    route.id
                )));
            };
            for domain in domains {
                if matcher_host.is_some_and(|host| host != domain) {
                    continue;
                }
                by_host
                    .entry(domain.clone())
                    .or_default()
                    .push(CompiledRoute {
                        priority: route.priority,
                        route_id: route.id.clone(),
                        upstream_pool_id: upstream_pool_id.clone(),
                        specificity: (
                            path_prefix
                                .as_ref()
                                .map_or(0, |path| path.as_str().len().saturating_sub(1)),
                            u8::from(matcher_host.is_some()),
                        ),
                        path_prefix: path_prefix.clone(),
                    });
            }
        }
        // Explicit priority is authoritative. Ties prefer the longer segment
        // prefix, then an explicit host constraint, then the stable route ID.
        for routes in by_host.values_mut() {
            routes.sort_by(|left, right| {
                (left.priority, Reverse(left.specificity), &left.route_id).cmp(&(
                    right.priority,
                    Reverse(right.specificity),
                    &right.route_id,
                ))
            });
        }
        Ok(Self { by_host })
    }

    pub(crate) fn select(&self, host: &NormalizedHost, path: &str) -> Option<ProxyRouteSelection> {
        if !path.starts_with('/') {
            return None;
        }
        self.by_host
            .get(host)?
            .iter()
            .find(|route| route.matches(path))
            .map(|route| ProxyRouteSelection {
                route_id: route.route_id.clone(),
                upstream_pool_id: route.upstream_pool_id.clone(),
            })
    }
}

impl CompiledRoute {
    fn matches(&self, path: &str) -> bool {
        self.path_prefix
            .as_ref()
            .is_none_or(|prefix| path_matches(prefix.as_str(), path))
    }
}

fn path_matches(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{PathPrefix, RevisionId};
    use panel_ir::{DomainSpec, RouteSpec, SiteSpec};

    fn site(id: &str, host: &str) -> SiteSpec {
        SiteSpec {
            id: SiteId::new(id).unwrap(),
            name: id.into(),
            enabled: true,
            domains: vec![DomainSpec {
                host: NormalizedHost::new(host).unwrap(),
                tls_profile_id: None,
            }],
        }
    }

    fn route(id: &str, priority: u32, matcher: RouteMatcher) -> RouteSpec {
        RouteSpec {
            id: RouteId::new(id).unwrap(),
            site_id: SiteId::new("site").unwrap(),
            priority,
            enabled: true,
            matcher,
            action: RouteAction::Proxy {
                upstream_pool_id: UpstreamPoolId::new(id).unwrap(),
            },
            retry_policy: None,
            header_policy_id: None,
            cache_policy_id: None,
            security_policy_id: None,
            lua_policy_id: None,
        }
    }

    #[test]
    fn priority_and_specificity_are_deterministic() {
        let host = NormalizedHost::new("example.com").unwrap();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", "example.com"));
        snapshot.routes = vec![
            route(
                "generic",
                10,
                RouteMatcher::PathPrefix {
                    path: PathPrefix::new("/").unwrap(),
                },
            ),
            route(
                "host-default",
                10,
                RouteMatcher::Host { host: host.clone() },
            ),
            route(
                "docs",
                10,
                RouteMatcher::PathPrefix {
                    path: PathPrefix::new("/docs").unwrap(),
                },
            ),
            route(
                "specific",
                10,
                RouteMatcher::HostPathPrefix {
                    host: host.clone(),
                    path: PathPrefix::new("/api").unwrap(),
                },
            ),
            route(
                "priority",
                1,
                RouteMatcher::PathPrefix {
                    path: PathPrefix::new("/admin").unwrap(),
                },
            ),
        ];
        let index = RouteIndex::compile(&snapshot).unwrap();
        let selected = |path| {
            index
                .select(&host, path)
                .unwrap()
                .route_id()
                .as_str()
                .to_owned()
        };
        assert_eq!(selected("/api/v1"), "specific");
        assert_eq!(selected("/admin"), "priority");
        assert_eq!(selected("/docs/page"), "docs");
        assert_eq!(selected("/other"), "host-default");
        assert_eq!(index.select(&host, "relative"), None);
    }

    #[test]
    fn prefix_respects_segment_boundary_and_disabled_resources() {
        let host = NormalizedHost::new("example.com").unwrap();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", "example.com"));
        snapshot.routes.push(route(
            "api",
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/api/").unwrap(),
            },
        ));
        let index = RouteIndex::compile(&snapshot).unwrap();
        assert!(index.select(&host, "/api").is_some());
        assert!(index.select(&host, "/api/users").is_some());
        assert!(index.select(&host, "/apiculture").is_none());
        snapshot.sites[0].enabled = false;
        assert!(RouteIndex::compile(&snapshot)
            .unwrap()
            .select(&host, "/api")
            .is_none());
    }

    #[test]
    fn host_matching_is_exact_and_disabled_routes_are_omitted() {
        let host = NormalizedHost::new("example.com").unwrap();
        let other = NormalizedHost::new("other.example").unwrap();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", "example.com"));
        snapshot
            .routes
            .push(route("host", 1, RouteMatcher::Host { host: host.clone() }));
        let index = RouteIndex::compile(&snapshot).unwrap();
        assert_eq!(
            index.select(&host, "/").unwrap().route_id().as_str(),
            "host"
        );
        assert!(index.select(&other, "/").is_none());
        snapshot.routes[0].enabled = false;
        assert!(RouteIndex::compile(&snapshot)
            .unwrap()
            .select(&host, "/")
            .is_none());
    }

    #[test]
    fn path_only_routes_are_scoped_to_their_site_domains() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites = vec![site("site", "example.com"), site("other", "other.example")];
        snapshot.routes.push(route(
            "api",
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/api").unwrap(),
            },
        ));
        let mut other_route = route(
            "other-api",
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/api").unwrap(),
            },
        );
        other_route.site_id = SiteId::new("other").unwrap();
        snapshot.routes.push(other_route);
        let index = RouteIndex::compile(&snapshot).unwrap();
        assert_eq!(
            index
                .select(&NormalizedHost::new("example.com").unwrap(), "/api")
                .unwrap()
                .route_id()
                .as_str(),
            "api"
        );
        assert_eq!(
            index
                .select(&NormalizedHost::new("other.example").unwrap(), "/api")
                .unwrap()
                .route_id()
                .as_str(),
            "other-api"
        );
        assert!(index
            .select(&NormalizedHost::new("unknown.example").unwrap(), "/api")
            .is_none());
    }

    #[test]
    fn unsupported_route_variants_fail_compilation() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", "example.com"));
        snapshot.routes.push(route(
            "api",
            1,
            RouteMatcher::ExactPath {
                path: "/api".into(),
            },
        ));
        assert_eq!(
            RouteIndex::compile(&snapshot).err().unwrap().code.as_str(),
            panel_errors::ErrorCode::UNSUPPORTED_CAPABILITY
        );

        snapshot.routes[0].matcher = RouteMatcher::Host {
            host: NormalizedHost::new("example.com").unwrap(),
        };
        snapshot.routes[0].action = RouteAction::Respond {
            status: 200,
            body: None,
        };
        assert_eq!(
            RouteIndex::compile(&snapshot).err().unwrap().code.as_str(),
            panel_errors::ErrorCode::UNSUPPORTED_CAPABILITY
        );
    }
}
