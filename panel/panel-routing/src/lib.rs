#![forbid(unsafe_code)]

//! Which site and route take a request (ADR 0036): one matcher, which the
//! gateway serves requests with and the control plane's route tester
//! explains requests with, so the two agree by construction.

mod conditions;
pub mod path;

use conditions::Condition;
use globset::{GlobBuilder, GlobMatcher};
use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId};
use panel_engine::ROUTE_REGEX_SIZE_LIMIT;
use panel_errors::{PanelError, Result};
use panel_ir::{RouteMatcher, RouteSpec, RuntimeSnapshot};
use regex::{Regex, RegexBuilder};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap},
    fmt,
    net::IpAddr,
};

/// A request as routing sees it.
pub trait Request {
    fn method(&self) -> &str;
    /// The host, normalized: lowercase and without a port.
    fn host(&self) -> &str;
    /// The path, normalized by [`path::normalize`].
    fn path(&self) -> &str;
    /// The query, without its `?`.
    fn query(&self) -> Option<&str>;
    /// The value of each field line of the header `name`, given in
    /// lowercase.
    fn header_lines(&self, name: &str) -> impl Iterator<Item = &[u8]>;
    /// The client's address, after trusted proxies.
    fn client(&self) -> Option<IpAddr>;
}

/// A request described by its parts, as the route tester takes it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimulatedRequest {
    pub method: String,
    pub host: String,
    pub path: String,
    pub query: Option<String>,
    /// Field lines in order, by name.
    pub headers: Vec<(String, String)>,
    pub client: Option<IpAddr>,
}

impl Request for SimulatedRequest {
    fn method(&self) -> &str {
        &self.method
    }

    fn host(&self) -> &str {
        &self.host
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    fn header_lines(&self, name: &str) -> impl Iterator<Item = &[u8]> {
        self.headers
            .iter()
            .filter(move |(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_bytes())
    }

    fn client(&self) -> Option<IpAddr> {
        self.client
    }
}

/// Where a host leads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEntry {
    pub site: usize,
    /// The configured domain that matched, among the router's `domains`.
    pub domain: usize,
    pub redirect_to_primary: bool,
}

/// The sites of a snapshot by host, and each site's routes in the order
/// they are tried.
pub struct Router {
    exact: HashMap<String, HostEntry>,
    /// Keyed by the parent of `*.parent`, so a lookup strips one label.
    wildcard: HashMap<String, HostEntry>,
    /// Every enabled domain as configured, such as `*.shop.example`.
    domains: Vec<String>,
    sites: Vec<SiteRouting>,
    default_sites: HashMap<String, usize>,
}

/// An enabled site and its enabled routes, ranked.
pub struct SiteRouting {
    pub id: SiteId,
    /// The site's place among the snapshot's sites.
    pub spec: usize,
    /// `None` serves every listener.
    listeners: Option<BTreeSet<String>>,
    routes: Vec<RouteMatch>,
}

/// What a route takes.
pub struct RouteMatch {
    pub id: RouteId,
    pub name: Option<String>,
    /// The route's place among the snapshot's routes.
    pub spec: usize,
    host: Option<NormalizedHost>,
    path: PathMatcher,
    conditions: Vec<Condition>,
}

/// Why a route does not take a request.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Mismatch {
    /// The route takes another host.
    Host(String),
    /// The route's path does not match, such as `prefix /api`.
    Path(String),
    /// A condition does not hold, with what the request had.
    Condition(String),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(host) => write!(formatter, "the route takes {host}"),
            Self::Path(path) => write!(formatter, "the path is not {path}"),
            Self::Condition(condition) => write!(formatter, "{condition} does not hold"),
        }
    }
}

enum PathMatcher {
    Any,
    Exact(String),
    Prefix(String),
    Glob(GlobMatcher),
    Regex(Regex),
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

impl Router {
    /// The enabled sites and routes of `snapshot`.
    pub fn compile(snapshot: &RuntimeSnapshot) -> Result<Self> {
        let mut router = Self {
            exact: HashMap::new(),
            wildcard: HashMap::new(),
            domains: Vec::new(),
            sites: Vec::new(),
            default_sites: HashMap::new(),
        };
        let mut site_indexes = HashMap::new();
        for (spec, site) in snapshot.sites.iter().enumerate() {
            if !site.enabled {
                continue;
            }
            let index = router.sites.len();
            site_indexes.insert(site.id.clone(), index);
            for domain in site.domains.iter().filter(|domain| domain.enabled) {
                let entry = HostEntry {
                    site: index,
                    domain: router.domains.len(),
                    redirect_to_primary: domain.redirect_to_primary,
                };
                router.domains.push(domain.host.as_str().to_owned());
                match domain.host.as_str().strip_prefix("*.") {
                    Some(parent) => router.wildcard.insert(parent.to_owned(), entry),
                    None => router.exact.insert(domain.host.as_str().to_owned(), entry),
                };
            }
            router.sites.push(SiteRouting {
                id: site.id.clone(),
                spec,
                listeners: (!site.listener_ids.is_empty()).then(|| site.listener_ids.clone()),
                routes: Vec::new(),
            });
        }
        for listener in &snapshot.listeners {
            if let Some(index) = listener
                .default_site_id
                .as_ref()
                .and_then(|site| site_indexes.get(site))
            {
                router.default_sites.insert(listener.id.clone(), *index);
            }
        }
        let mut ranked: Vec<Vec<(RouteRank, RouteMatch)>> =
            router.sites.iter().map(|_| Vec::new()).collect();
        for (spec, route) in snapshot.routes.iter().enumerate() {
            let Some(index) = route
                .enabled
                .then(|| site_indexes.get(&route.site_id))
                .flatten()
            else {
                continue;
            };
            let compiled = RouteMatch::compile(route, spec)?;
            ranked[*index].push((compiled.rank(route.priority), compiled));
        }
        for (site, mut routes) in router.sites.iter_mut().zip(ranked) {
            routes.sort_by(|left, right| left.0.cmp(&right.0));
            site.routes = routes.into_iter().map(|(_, route)| route).collect();
        }
        Ok(router)
    }

    /// The site a host leads to, an exact name before a wildcard.
    pub fn lookup(&self, host: &str) -> Option<HostEntry> {
        self.exact.get(host).copied().or_else(|| {
            host.split_once('.')
                .and_then(|(_, parent)| self.wildcard.get(parent).copied())
        })
    }

    pub fn domains(&self) -> &[String] {
        &self.domains
    }

    pub fn site(&self, index: usize) -> &SiteRouting {
        &self.sites[index]
    }

    pub fn sites(&self) -> &[SiteRouting] {
        &self.sites
    }

    /// The site a listener serves hosts no site names with.
    pub fn default_site(&self, listener: &str) -> Option<usize> {
        self.default_sites.get(listener).copied()
    }
}

impl SiteRouting {
    pub fn serves(&self, listener: &str) -> bool {
        self.listeners
            .as_ref()
            .is_none_or(|listeners| listeners.contains(listener))
    }

    /// The first route that takes `request`.
    pub fn select(&self, request: &impl Request) -> Option<usize> {
        self.routes.iter().position(|route| route.matches(request))
    }

    pub fn routes(&self) -> &[RouteMatch] {
        &self.routes
    }

    pub fn route(&self, index: usize) -> &RouteMatch {
        &self.routes[index]
    }
}

impl RouteMatch {
    fn compile(route: &RouteSpec, spec: usize) -> Result<Self> {
        let (host, path) = compile_matcher(&route.id, &route.matcher)?;
        let conditions = route
            .conditions
            .iter()
            .map(Condition::compile)
            .collect::<Result<_>>()
            .map_err(|error| {
                PanelError::validation_failed(format!("route {}: {}", route.id, error.message))
            })?;
        Ok(Self {
            id: route.id.clone(),
            name: route.name.clone(),
            spec,
            host,
            path,
            conditions,
        })
    }

    fn rank(&self, priority: u32) -> RouteRank {
        RouteRank {
            priority,
            specificity: Reverse(self.path.specificity()),
            host: Reverse(
                self.host
                    .as_ref()
                    .map_or(0, |host| if host.is_wildcard() { 1 } else { 2 }),
            ),
            id: self.id.clone(),
        }
    }

    /// Whether the route takes `request`: its host, its path, then every
    /// condition.
    pub fn matches(&self, request: &impl Request) -> bool {
        self.host
            .as_ref()
            .is_none_or(|host| host_matches(host, request.host()))
            && self.path.matches(request.path())
            && self
                .conditions
                .iter()
                .all(|condition| condition.holds(request))
    }

    /// The first part that keeps the route from taking `request`.
    pub fn mismatch(&self, request: &impl Request) -> Option<Mismatch> {
        if let Some(host) = self
            .host
            .as_ref()
            .filter(|host| !host_matches(host, request.host()))
        {
            return Some(Mismatch::Host(host.as_str().to_owned()));
        }
        if !self.path.matches(request.path()) {
            return Some(Mismatch::Path(self.path.to_string()));
        }
        self.conditions
            .iter()
            .find_map(|condition| condition.mismatch(request))
            .map(Mismatch::Condition)
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

impl fmt::Display for PathMatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => formatter.write_str("prefix /"),
            Self::Exact(path) => write!(formatter, "exact {path}"),
            Self::Prefix(prefix) => write!(formatter, "prefix {prefix}"),
            Self::Glob(glob) => write!(formatter, "glob {}", glob.glob().glob()),
            Self::Regex(regex) => write!(formatter, "regex {}", regex.as_str()),
        }
    }
}

/// Whether `host` is `pattern`, or one label below a `*.parent` pattern.
pub(crate) fn host_matches(pattern: &NormalizedHost, host: &str) -> bool {
    match pattern.as_str().strip_prefix('*') {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|label| !label.is_empty() && !label.contains('.')),
        None => pattern.as_str() == host,
    }
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
            let normalized = path::normalize(path)
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

#[cfg(test)]
mod tests;
