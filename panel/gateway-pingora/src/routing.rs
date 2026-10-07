//! Immutable routing tables compiled before a snapshot becomes active:
//! `panel-routing` decides which site and route take a request, and the
//! table keeps what serving them needs.

use crate::{
    access_log::AccessPlan,
    error_pages::{MaintenancePlan, PageSet},
    lua::{Hook, HookIndex, Hooks},
    rewrite::{InternalTarget, Rewrites},
    template::Template,
};
use http::HeaderValue;
use panel_domain::{RouteId, SiteId};
use panel_errors::{PanelError, Result};
use panel_ir::{RouteAction, RuntimeSnapshot, WwwRedirect};
use panel_routing::{HostEntry, Request, Router};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
};

const HTTPS_PORT: u16 = 443;

pub(crate) struct RoutingTable {
    /// How requests no site takes are logged.
    access: AccessPlan,
    router: Router,
    /// Aligned with the router's sites.
    sites: Vec<SiteRoutes>,
}

pub(crate) struct SiteRoutes {
    pub id: SiteId,
    pub https_redirect: bool,
    /// Port of the TLS listener used for HTTPS redirects; `None` is 443.
    pub https_port: Option<u16>,
    pub primary: Option<String>,
    /// `Strict-Transport-Security` for its HTTPS responses.
    pub hsts: Option<HeaderValue>,
    /// The security policy every request for the site passes.
    pub security: Option<usize>,
    /// The HTTP policy every request for the site passes, before its
    /// route's.
    pub http: Option<usize>,
    /// How requests the site takes without a route are logged.
    pub access: AccessPlan,
    /// Lua hooks for the site's requests; `server_rewrite` runs before the
    /// route is chosen.
    pub lua: Hooks,
    /// Rules every request runs before a route is chosen.
    pub rewrites: Rewrites,
    /// Pages answering errors of requests whose route has none.
    pub error_pages: PageSet,
    /// While set, only the clients it admits reach the site.
    pub maintenance: Option<MaintenancePlan>,
    www: HashMap<String, String>,
    /// Aligned with the router's ranked routes of the site.
    routes: Vec<CompiledRoute>,
}

pub(crate) struct CompiledRoute {
    pub id: RouteId,
    pub name: Option<String>,
    pub target: RouteTarget,
    /// The security policy the route's requests pass after the site's.
    pub security: Option<usize>,
    /// The HTTP policy the route's requests pass after the site's.
    pub http: Option<usize>,
    pub access: AccessPlan,
    /// The Lua hooks the route's requests run, inheritance resolved.
    pub lua: Hooks,
    /// Rules the route's requests run once it is chosen.
    pub rewrites: Rewrites,
    /// Takes only requests sent to it from inside the gateway.
    pub internal: bool,
    /// Pages answering the route's errors in place of its site's.
    pub error_pages: Option<PageSet>,
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
    /// `content_by_lua`.
    Lua(Hook),
    InternalRedirect(InternalTarget),
}

/// Resolves IR references to compiled pool and static content indexes.
pub(crate) struct Targets<'a> {
    pub pools: &'a HashMap<&'a str, usize>,
    pub statics: &'a HashMap<&'a str, usize>,
    pub policies: &'a HashMap<&'a str, usize>,
    pub http: &'a HashMap<&'a str, usize>,
    pub lua: &'a HookIndex,
    /// Where static content and error page files are below.
    pub static_root: Option<&'a std::path::Path>,
}

fn http_policy(
    targets: &Targets<'_>,
    owner: &dyn std::fmt::Display,
    id: Option<&String>,
) -> Result<Option<usize>> {
    id.map(|id| {
        targets.http.get(id.as_str()).copied().ok_or_else(|| {
            PanelError::validation_failed(format!("{owner} names an unknown HTTP policy {id}"))
        })
    })
    .transpose()
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
        let router = Router::compile(snapshot)?;
        let mut sites = Vec::with_capacity(router.sites().len());
        for routing in router.sites() {
            let site = &snapshot.sites[routing.spec];
            let listeners = (!site.listener_ids.is_empty()).then_some(&site.listener_ids);
            let https_port = snapshot
                .listeners
                .iter()
                .filter(|listener| listener.tls_profile_id.is_some())
                .filter(|listener| listeners.is_none_or(|ids| ids.contains(&listener.id)))
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
            let routes = routing
                .routes()
                .iter()
                .map(|matched| {
                    let route = &snapshot.routes[matched.spec];
                    Ok(CompiledRoute {
                        id: route.id.clone(),
                        name: route.name.clone(),
                        target: compile_target(&route.id, &route.action, targets)?,
                        security: policy(
                            targets,
                            &format!("route {}", route.id),
                            route.security_policy_id.as_ref(),
                        )?,
                        http: http_policy(
                            targets,
                            &format!("route {}", route.id),
                            route.header_policy_id.as_ref(),
                        )?,
                        access: AccessPlan::resolve(&[
                            &snapshot.logging.access,
                            &site.access_log,
                            &route.access_log,
                        ])?,
                        lua: targets
                            .lua
                            .routes
                            .get(route.id.as_str())
                            .cloned()
                            .unwrap_or_default(),
                        rewrites: Rewrites::compile(&route.rewrites).map_err(|error| {
                            rewrite_error(&format!("route {}", route.id), &error)
                        })?,
                        internal: route.internal,
                        error_pages: route
                            .error_pages
                            .as_ref()
                            .map(|pages| {
                                PageSet::compile(
                                    pages,
                                    targets.static_root,
                                    &format!("route {}", route.id),
                                )
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<_>>()?;
            sites.push(SiteRoutes {
                id: site.id.clone(),
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
                security: policy(
                    targets,
                    &format!("site {}", site.id),
                    site.security_policy_id.as_ref(),
                )?,
                http: http_policy(
                    targets,
                    &format!("site {}", site.id),
                    site.header_policy_id.as_ref(),
                )?,
                access: AccessPlan::resolve(&[&snapshot.logging.access, &site.access_log])?,
                lua: targets
                    .lua
                    .sites
                    .get(site.id.as_str())
                    .cloned()
                    .unwrap_or_default(),
                rewrites: Rewrites::compile(&site.rewrites)
                    .map_err(|error| rewrite_error(&format!("site {}", site.id), &error))?,
                error_pages: PageSet::compile(
                    &site.error_pages,
                    targets.static_root,
                    &format!("site {}", site.id),
                )?,
                maintenance: site
                    .maintenance
                    .as_ref()
                    .map(|maintenance| {
                        MaintenancePlan::compile(maintenance, &format!("site {}", site.id))
                    })
                    .transpose()?,
                www,
                routes,
            });
        }
        Ok(Self {
            access: AccessPlan::resolve(&[&snapshot.logging.access])?,
            router,
            sites,
        })
    }

    /// How a request that `site` and `route` took, if any, is logged.
    pub(crate) fn access(&self, site: Option<usize>, route: Option<usize>) -> &AccessPlan {
        let Some(site) = site.and_then(|site| self.sites.get(site)) else {
            return &self.access;
        };
        route
            .and_then(|route| site.routes.get(route))
            .map_or(&site.access, |route| &route.access)
    }

    pub(crate) fn lookup(&self, host: &str) -> Option<HostEntry> {
        self.router.lookup(host)
    }

    pub(crate) fn domains(&self) -> &[String] {
        self.router.domains()
    }

    pub(crate) fn site(&self, index: usize) -> &SiteRoutes {
        &self.sites[index]
    }

    pub(crate) fn sites(&self) -> &[SiteRoutes] {
        &self.sites
    }

    pub(crate) fn default_site(&self, listener: &str) -> Option<usize> {
        self.router.default_site(listener)
    }

    /// Whether the site at `site` serves `listener`.
    pub(crate) fn serves(&self, site: usize, listener: &str) -> bool {
        self.router.site(site).serves(listener)
    }

    /// The first route of the site at `site` that takes `request`.
    pub(crate) fn select(&self, site: usize, request: &impl Request) -> Option<usize> {
        self.router.site(site).select(request)
    }

    /// The named location `name` of the site at `site`.
    pub(crate) fn named(&self, site: usize, name: &str) -> Option<usize> {
        self.router.site(site).named(name)
    }
}

impl SiteRoutes {
    pub(crate) fn www_target(&self, host: &str) -> Option<&str> {
        self.www.get(host).map(String::as_str)
    }

    pub(crate) fn routes(&self) -> &[CompiledRoute] {
        &self.routes
    }

    pub(crate) fn route(&self, index: usize) -> &CompiledRoute {
        &self.routes[index]
    }
}

fn template_error(route: &RouteId, error: &str) -> PanelError {
    PanelError::validation_failed(format!("route {route} has an invalid template: {error}"))
}

fn rewrite_error(owner: &str, error: &str) -> PanelError {
    PanelError::validation_failed(format!(
        "{owner} has a rewrite that does not compile: {error}"
    ))
}

fn compile_target(
    route: &RouteId,
    action: &RouteAction,
    targets: &Targets<'_>,
) -> Result<RouteTarget> {
    Ok(match action {
        RouteAction::Lua { .. } => RouteTarget::Lua(
            targets
                .lua
                .contents
                .get(route.as_str())
                .cloned()
                .ok_or_else(|| {
                    PanelError::validation_failed(format!(
                        "route {route} has a Lua handler that was not compiled"
                    ))
                })?,
        ),
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
        RouteAction::InternalRedirect { target } => RouteTarget::InternalRedirect(
            InternalTarget::compile(target).map_err(|error| template_error(route, &error))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{NormalizedHost, RevisionId, UpstreamPoolId};
    use panel_ir::{DomainSpec, ListenerRef, RouteMatcher, RouteSpec, SiteSpec};
    use panel_routing::SimulatedRequest;

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

    fn compile(snapshot: &RuntimeSnapshot) -> Result<RoutingTable> {
        let pools = HashMap::from([("pool", 0)]);
        let statics = HashMap::from([("static", 0)]);
        let policies = HashMap::new();
        let http = HashMap::new();
        RoutingTable::compile(
            snapshot,
            &Targets {
                pools: &pools,
                statics: &statics,
                policies: &policies,
                http: &http,
                lua: &HookIndex::default(),
                static_root: None,
            },
        )
    }

    #[test]
    fn serving_data_follows_the_routes_the_router_ranks() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", &["example.com"]));
        let route = |id: &str, priority, path: &str| {
            RouteSpec::new(
                RouteId::new(id).unwrap(),
                SiteId::new("site").unwrap(),
                priority,
                RouteMatcher::PathPrefix {
                    path: panel_domain::PathPrefix::new(path).unwrap(),
                },
                RouteAction::Proxy {
                    upstream_pool_id: UpstreamPoolId::new("pool").unwrap(),
                },
            )
        };
        snapshot.routes = vec![route("late", 20, "/"), route("early", 10, "/api")];
        let table = compile(&snapshot).unwrap();
        let request = SimulatedRequest {
            method: "GET".into(),
            host: "example.com".into(),
            path: "/api/items".into(),
            ..SimulatedRequest::default()
        };
        let site = table.lookup("example.com").unwrap().site;
        let index = table.select(site, &request).unwrap();
        assert_eq!(table.site(site).route(index).id.as_str(), "early");
        assert_eq!(table.site(site).routes()[1].id.as_str(), "late");
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
        let internal = table.lookup("internal.example").unwrap().site;
        assert!(table.serves(internal, "https") && !table.serves(internal, "http"));
    }
}
