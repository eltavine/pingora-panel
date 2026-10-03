//! Immutable routing tables compiled before a snapshot becomes active.

use crate::template::Template;
use globset::{GlobBuilder, GlobMatcher};
use http::HeaderValue;
use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId};
use panel_engine::ROUTE_REGEX_SIZE_LIMIT;
use panel_errors::{PanelError, Result};
use panel_ir::{RouteAction, RouteMatcher, RuntimeSnapshot, WwwRedirect};
use regex::{Regex, RegexBuilder};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
};

const HTTPS_PORT: u16 = 443;

pub(crate) struct RoutingTable {
    exact: HashMap<String, HostEntry>,
    /// Keyed by the parent of `*.parent`, so a lookup strips one label.
    wildcard: HashMap<String, HostEntry>,
    sites: Vec<SiteRoutes>,
    default_sites: HashMap<String, usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HostEntry {
    pub site: usize,
    pub redirect_to_primary: bool,
}

pub(crate) struct SiteRoutes {
    pub id: SiteId,
    /// `None` serves every listener.
    listeners: Option<BTreeSet<String>>,
    pub https_redirect: bool,
    /// Port of the TLS listener used for HTTPS redirects; `None` is 443.
    pub https_port: Option<u16>,
    pub primary: Option<String>,
    /// `Strict-Transport-Security` for its HTTPS responses.
    pub hsts: Option<HeaderValue>,
    /// The security policy every request for the site passes.
    pub security: Option<usize>,
    www: HashMap<String, String>,
    routes: Vec<CompiledRoute>,
}

pub(crate) struct CompiledRoute {
    pub id: RouteId,
    pub name: Option<String>,
    host: Option<NormalizedHost>,
    path: PathMatcher,
    pub target: RouteTarget,
    /// The security policy the route's requests pass after the site's.
    pub security: Option<usize>,
}

enum PathMatcher {
    Any,
    Exact(String),
    Prefix(String),
    Glob(GlobMatcher),
    Regex(Regex),
}

pub(crate) enum RouteTarget {
    Proxy(usize),
    Static(usize),
    Redirect {
        location: Template,
        status: u16,
        preserve_path: bool,
    },
    Respond {
        status: u16,
        body: Template,
        content_type: Option<String>,
        retry_after: Option<u32>,
    },
}

/// Resolves IR references to compiled pool and static content indexes.
pub(crate) struct Targets<'a> {
    pub pools: &'a HashMap<&'a str, usize>,
    pub statics: &'a HashMap<&'a str, usize>,
    pub policies: &'a HashMap<&'a str, usize>,
}

fn policy(
    targets: &Targets<'_>,
    owner: &dyn std::fmt::Display,
    id: Option<&String>,
) -> Result<Option<usize>> {
    id.map(|id| {
        targets.policies.get(id.as_str()).copied().ok_or_else(|| {
            PanelError::validation_failed(format!("{owner} names an unknown security policy {id}"))
        })
    })
    .transpose()
}

impl RoutingTable {
    pub(crate) fn compile(snapshot: &RuntimeSnapshot, targets: &Targets<'_>) -> Result<Self> {
        let mut table = Self {
            exact: HashMap::new(),
            wildcard: HashMap::new(),
            sites: Vec::new(),
            default_sites: HashMap::new(),
        };
        let mut site_indexes = HashMap::new();
        for site in snapshot.sites.iter().filter(|site| site.enabled) {
            let index = table.sites.len();
            site_indexes.insert(site.id.clone(), index);
            let listeners = (!site.listener_ids.is_empty()).then(|| site.listener_ids.clone());
            let https_port = snapshot
                .listeners
                .iter()
                .filter(|listener| listener.tls_profile_id.is_some())
                .filter(|listener| {
                    listeners
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&listener.id))
                })
                .min_by(|left, right| left.id.cmp(&right.id))
                .and_then(|listener| listener.address.parse::<SocketAddr>().ok())
                .map(|address| address.port())
                .filter(|port| *port != HTTPS_PORT);
            let domains: Vec<_> = site
                .domains
                .iter()
                .filter(|domain| domain.enabled)
                .collect();
            let names: BTreeSet<&str> = domains
                .iter()
                .filter(|domain| !domain.host.is_wildcard())
                .map(|domain| domain.host.as_str())
                .collect();
            let mut www = HashMap::new();
            for name in &names {
                let target = match site.www_redirect {
                    WwwRedirect::None => None,
                    WwwRedirect::AddWww => Some(format!("www.{name}")).filter(|target| {
                        !name.starts_with("www.") && names.contains(target.as_str())
                    }),
                    WwwRedirect::RemoveWww => name
                        .strip_prefix("www.")
                        .filter(|target| names.contains(target))
                        .map(str::to_owned),
                };
                if let Some(target) = target {
                    www.insert((*name).to_owned(), target);
                }
            }
            for domain in &domains {
                let entry = HostEntry {
                    site: index,
                    redirect_to_primary: domain.redirect_to_primary,
                };
                match domain.host.as_str().strip_prefix("*.") {
                    Some(parent) => table.wildcard.insert(parent.to_owned(), entry),
                    None => table.exact.insert(domain.host.as_str().to_owned(), entry),
                };
            }
            table.sites.push(SiteRoutes {
                id: site.id.clone(),
                listeners,
                https_redirect: site.https_redirect,
                https_port,
                primary: domains
                    .iter()
                    .find(|domain| domain.primary)
                    .map(|domain| domain.host.as_str().to_owned()),
                hsts: site
                    .hsts
                    .map(|policy| HeaderValue::from_str(&policy.header_value()))
                    .transpose()
                    .map_err(|_| {
                        PanelError::validation_failed(format!(
                            "site {} has an invalid HSTS policy",
                            site.id
                        ))
                    })?,
                www,
                routes: Vec::new(),
                security: policy(
                    targets,
                    &format!("site {}", site.id),
                    site.security_policy_id.as_ref(),
                )?,
            });
        }
        for listener in &snapshot.listeners {
            if let Some(index) = listener
                .default_site_id
                .as_ref()
                .and_then(|site| site_indexes.get(site))
            {
                table.default_sites.insert(listener.id.clone(), *index);
            }
        }
        let mut ranked: Vec<Vec<(RouteRank, CompiledRoute)>> =
            table.sites.iter().map(|_| Vec::new()).collect();
        for route in snapshot.routes.iter().filter(|route| route.enabled) {
            let Some(index) = site_indexes.get(&route.site_id) else {
                continue;
            };
            let (host, path) = compile_matcher(&route.id, &route.matcher)?;
            let target = compile_target(&route.id, &route.action, targets)?;
            let rank = RouteRank {
                priority: route.priority,
                specificity: Reverse(path.specificity()),
                host: Reverse(
                    host.as_ref()
                        .map_or(0, |host| if host.is_wildcard() { 1 } else { 2 }),
                ),
                id: route.id.clone(),
            };
            ranked[*index].push((
                rank,
                CompiledRoute {
                    id: route.id.clone(),
                    name: route.name.clone(),
                    host,
                    path,
                    target,
                    security: policy(
                        targets,
                        &format!("route {}", route.id),
                        route.security_policy_id.as_ref(),
                    )?,
                },
            ));
        }
        for (site, mut routes) in table.sites.iter_mut().zip(ranked) {
            routes.sort_by(|left, right| left.0.cmp(&right.0));
            site.routes = routes.into_iter().map(|(_, route)| route).collect();
        }
        Ok(table)
    }

    pub(crate) fn lookup(&self, host: &str) -> Option<HostEntry> {
        self.exact.get(host).copied().or_else(|| {
            host.split_once('.')
                .and_then(|(_, parent)| self.wildcard.get(parent).copied())
        })
    }

    pub(crate) fn site(&self, index: usize) -> &SiteRoutes {
        &self.sites[index]
    }

    pub(crate) fn default_site(&self, listener: &str) -> Option<usize> {
        self.default_sites.get(listener).copied()
    }
}

/// Explicit priority is authoritative. Ties prefer the more specific path
/// matcher, then a concrete host constraint, then the stable route ID.
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct RouteRank {
    priority: u32,
    specificity: Reverse<(u8, usize)>,
    host: Reverse<u8>,
    id: RouteId,
}

impl SiteRoutes {
    pub(crate) fn serves(&self, listener: &str) -> bool {
        self.listeners
            .as_ref()
            .is_none_or(|listeners| listeners.contains(listener))
    }

    pub(crate) fn www_target(&self, host: &str) -> Option<&str> {
        self.www.get(host).map(String::as_str)
    }

    /// `path` must already be normalized.
    pub(crate) fn select(&self, host: &str, path: &str) -> Option<usize> {
        self.routes.iter().position(|route| {
            route
                .host
                .as_ref()
                .is_none_or(|constraint| host_matches(constraint, host))
                && route.path.matches(path)
        })
    }

    pub(crate) fn route(&self, index: usize) -> &CompiledRoute {
        &self.routes[index]
    }
}

impl PathMatcher {
    /// Exact, then glob, then regex, then prefix; longer patterns first.
    fn specificity(&self) -> (u8, usize) {
        match self {
            Self::Exact(path) => (4, path.len()),
            Self::Glob(glob) => (3, glob.glob().glob().len()),
            Self::Regex(regex) => (2, regex.as_str().len()),
            Self::Prefix(prefix) => (1, prefix.len()),
            Self::Any => (0, 0),
        }
    }

    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(exact) => path == exact,
            Self::Prefix(prefix) => {
                path == prefix
                    || path
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|remainder| remainder.starts_with('/'))
            }
            Self::Glob(glob) => glob.is_match(path),
            Self::Regex(regex) => regex.is_match(path),
        }
    }
}

fn host_matches(pattern: &NormalizedHost, host: &str) -> bool {
    match pattern.as_str().strip_prefix('*') {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|label| !label.is_empty() && !label.contains('.')),
        None => pattern.as_str() == host,
    }
}

fn template_error(route: &RouteId, error: &str) -> PanelError {
    PanelError::validation_failed(format!("route {route} has an invalid template: {error}"))
}

fn compile_matcher(
    route: &RouteId,
    matcher: &RouteMatcher,
) -> Result<(Option<NormalizedHost>, PathMatcher)> {
    let invalid = |detail: String| {
        PanelError::validation_failed(format!("route {route} matcher is invalid: {detail}"))
    };
    // A "/" prefix matches every path, so it ranks like a host-only route.
    let prefix = |path: &PathPrefix| match path.as_str() {
        "/" => PathMatcher::Any,
        prefix => PathMatcher::Prefix(prefix.to_owned()),
    };
    Ok(match matcher {
        RouteMatcher::Host { host } => (Some(host.clone()), PathMatcher::Any),
        RouteMatcher::PathPrefix { path } => (None, prefix(path)),
        RouteMatcher::HostPathPrefix { host, path } => (Some(host.clone()), prefix(path)),
        RouteMatcher::ExactPath { path } => {
            let normalized = crate::path::normalize(path)
                .ok_or_else(|| invalid("exact path must be absolute".into()))?;
            (None, PathMatcher::Exact(normalized.into_owned()))
        }
        RouteMatcher::Glob { pattern } => {
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .map_err(|error| invalid(error.to_string()))?;
            (None, PathMatcher::Glob(glob.compile_matcher()))
        }
        RouteMatcher::Regex { pattern } => {
            let regex = RegexBuilder::new(pattern)
                .size_limit(ROUTE_REGEX_SIZE_LIMIT)
                .dfa_size_limit(ROUTE_REGEX_SIZE_LIMIT)
                .build()
                .map_err(|error| invalid(error.to_string()))?;
            (None, PathMatcher::Regex(regex))
        }
    })
}

fn compile_target(
    route: &RouteId,
    action: &RouteAction,
    targets: &Targets<'_>,
) -> Result<RouteTarget> {
    Ok(match action {
        RouteAction::Proxy { upstream_pool_id } => RouteTarget::Proxy(
            *targets
                .pools
                .get(upstream_pool_id.as_str())
                .ok_or_else(|| {
                    PanelError::validation_failed(format!(
                        "route {route} references unknown upstream pool {upstream_pool_id}"
                    ))
                })?,
        ),
        RouteAction::Static { policy_id } => {
            RouteTarget::Static(*targets.statics.get(policy_id.as_str()).ok_or_else(|| {
                PanelError::validation_failed(format!(
                    "route {route} references unknown static content {policy_id}"
                ))
            })?)
        }
        RouteAction::Redirect {
            location,
            status,
            preserve_path,
        } => RouteTarget::Redirect {
            location: Template::parse(location).map_err(|error| template_error(route, &error))?,
            status: *status,
            preserve_path: *preserve_path,
        },
        RouteAction::Respond {
            status,
            body,
            content_type,
            retry_after_seconds,
        } => RouteTarget::Respond {
            status: *status,
            body: Template::parse(body.as_deref().unwrap_or_default())
                .map_err(|error| template_error(route, &error))?,
            content_type: content_type.clone(),
            retry_after: *retry_after_seconds,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{RevisionId, UpstreamPoolId};
    use panel_ir::{DomainSpec, ListenerRef, RouteSpec, SiteSpec};

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

    fn compile(snapshot: &RuntimeSnapshot) -> Result<RoutingTable> {
        let pools = HashMap::from([("pool", 0)]);
        let statics = HashMap::from([("static", 0)]);
        let policies = HashMap::new();
        RoutingTable::compile(
            snapshot,
            &Targets {
                pools: &pools,
                statics: &statics,
                policies: &policies,
            },
        )
    }

    fn selected(table: &RoutingTable, host: &str, path: &str) -> Option<String> {
        let site = table.site(table.lookup(host)?.site);
        site.select(host, path)
            .map(|index| site.route(index).id.as_str().to_owned())
    }

    #[test]
    fn priority_then_specificity_decide() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", &["example.com"]));
        let host = NormalizedHost::new("example.com").unwrap();
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
                    host,
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
        let table = compile(&snapshot).unwrap();
        let select = |path| selected(&table, "example.com", path).unwrap();
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
        snapshot.routes.push(route(
            "api",
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/api/").unwrap(),
            },
        ));
        let table = compile(&snapshot).unwrap();
        assert!(selected(&table, "example.com", "/api").is_some());
        assert!(selected(&table, "example.com", "/api/users").is_some());
        assert!(selected(&table, "example.com", "/apiculture").is_none());
    }

    #[test]
    fn wildcard_domains_cover_one_label_and_exact_names_win() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites = vec![
            site("wild", &["*.example.com"]),
            site("api", &["api.example.com"]),
        ];
        let table = compile(&snapshot).unwrap();
        let site_of = |host| {
            table
                .lookup(host)
                .map(|entry| table.site(entry.site).id.as_str().to_owned())
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
        snapshot.routes.push(route(
            "all",
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").unwrap(),
            },
        ));
        let table = compile(&snapshot).unwrap();
        assert!(table.lookup("old.example.com").is_none());
        assert!(selected(&table, "example.com", "/").is_some());
        snapshot.routes[0].enabled = false;
        assert!(selected(&compile(&snapshot).unwrap(), "example.com", "/").is_none());
        snapshot.sites[0].enabled = false;
        assert!(compile(&snapshot).unwrap().lookup("example.com").is_none());
    }

    #[test]
    fn listener_scope_default_site_and_redirect_data() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut https = ListenerRef::new("https", "0.0.0.0:8443");
        https.tls_profile_id = Some("tls".into());
        let mut http = ListenerRef::new("http", "0.0.0.0:80");
        http.default_site_id = Some(SiteId::new("site").unwrap());
        snapshot.listeners = vec![http, https];
        let mut main = site(
            "site",
            &["example.com", "www.example.com", "alias.example.net"],
        );
        main.domains[0].primary = true;
        main.domains[2].redirect_to_primary = true;
        main.www_redirect = WwwRedirect::RemoveWww;
        main.https_redirect = true;
        snapshot.sites.push(main);
        let mut internal = site("internal", &["internal.example"]);
        internal.listener_ids.insert("https".into());
        snapshot.sites.push(internal);
        let table = compile(&snapshot).unwrap();
        let site = table.site(table.lookup("example.com").unwrap().site);
        assert_eq!(site.https_port, Some(8443));
        assert_eq!(site.primary.as_deref(), Some("example.com"));
        assert_eq!(site.www_target("www.example.com"), Some("example.com"));
        assert_eq!(site.www_target("example.com"), None);
        assert!(
            table
                .lookup("alias.example.net")
                .unwrap()
                .redirect_to_primary
        );
        assert_eq!(table.default_site("http"), Some(0));
        assert_eq!(table.default_site("https"), None);
        let internal = table.site(table.lookup("internal.example").unwrap().site);
        assert!(internal.serves("https") && !internal.serves("http"));
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
        assert!(compile(&snapshot).is_err());
        snapshot.routes[0].matcher = RouteMatcher::Glob {
            pattern: "/[".into(),
        };
        assert!(compile(&snapshot).is_err());
        snapshot.routes[0].matcher = RouteMatcher::Regex {
            pattern: "a{1000}{1000}".into(),
        };
        assert!(compile(&snapshot).is_err());
    }
}
