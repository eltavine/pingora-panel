//! Compiles the editable model into an engine-neutral runtime snapshot.

use crate::model::{Action, ConfigModel, MatchKind, Route, Site, TlsProfile};
use panel_domain::{
    EndpointAddress, EndpointId, PathPrefix, RevisionId, RouteId, SiteId, UpstreamPoolId,
};
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::template::{uses_variables, TEMPLATE_CAPABILITY};
use panel_ir::{
    CapabilityRequirement, DomainSpec, ListenerRef, LoadBalancingPolicy, RetryPolicy, RouteAction,
    RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec, StaticContentPolicy, UpstreamEndpoint,
    UpstreamPoolSpec, WwwRedirect,
};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The site action runs after every route the operator defined.
const SITE_ACTION_PRIORITY: u32 = u32::MAX;

/// Compiles live sites and the upstreams they use; the result still needs
/// engine validation before it can be prepared.
pub fn compile(
    model: &ConfigModel,
    revision: RevisionId,
) -> Result<RuntimeSnapshot, Vec<Diagnostic>> {
    let diagnostics = crate::validate::validate(model);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let mut compiler = Compiler {
        snapshot: RuntimeSnapshot::empty(revision),
        capabilities: BTreeSet::new(),
        diagnostics: Vec::new(),
    };
    let live: Vec<&Site> = model
        .sites
        .iter()
        .filter(|site| !site.is_deleted())
        .collect();
    let used: BTreeSet<Uuid> = live
        .iter()
        .flat_map(|site| {
            std::iter::once(&site.action)
                .chain(site.routes.iter().map(|route| &route.action))
                .filter_map(|action| match action {
                    Action::Proxy { upstream_id } => Some(*upstream_id),
                    _ => None,
                })
        })
        .collect();
    for listener in &model.listeners {
        compiler.listener(listener, &live);
    }
    compiler.snapshot.tls_profiles = model.tls_profiles.iter().map(TlsProfile::runtime).collect();
    for upstream in model
        .upstreams
        .iter()
        .filter(|upstream| used.contains(&upstream.id))
    {
        compiler.upstream(upstream);
    }
    for site in live {
        compiler.site(site);
    }
    if !compiler.diagnostics.is_empty() {
        return Err(compiler.diagnostics);
    }
    let mut snapshot = compiler.snapshot;
    snapshot.required_capabilities = compiler
        .capabilities
        .into_iter()
        .map(|name| CapabilityRequirement::new(name, "1"))
        .collect();
    snapshot.refresh_content_hash();
    Ok(snapshot)
}

struct Compiler {
    snapshot: RuntimeSnapshot,
    capabilities: BTreeSet<&'static str>,
    diagnostics: Vec<Diagnostic>,
}

impl Compiler {
    fn fail(&mut self, resource: String, message: String) {
        self.diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    }

    fn listener(&mut self, listener: &crate::model::Listener, live: &[&Site]) {
        self.capabilities
            .insert(if listener.tls_profile_id.is_some() {
                "listener.https"
            } else {
                "listener.http"
            });
        if listener.protocols.http2 {
            self.capabilities.insert("listener.http2");
        }
        if listener.protocols.http3 {
            self.capabilities.insert("listener.http3");
        }
        let mut compiled = ListenerRef::new(listener.id.clone(), listener.address.clone());
        compiled.tls_profile_id.clone_from(&listener.tls_profile_id);
        compiled.protocols = listener.protocols;
        compiled.reuse_port = listener.reuse_port;
        compiled.ipv6_only = listener.ipv6_only;
        compiled.default_site_id = listener
            .default_site_id
            .filter(|id| live.iter().any(|site| site.id == *id))
            .map(site_id);
        self.snapshot.listeners.push(compiled);
    }

    fn upstream(&mut self, upstream: &crate::model::Upstream) {
        let mut endpoints = Vec::with_capacity(upstream.nodes.len());
        for node in &upstream.nodes {
            let Ok(address) = EndpointAddress::new(&node.host, node.port, node.tls) else {
                self.fail(
                    format!("upstreams/{}/nodes/{}", upstream.id, node.id),
                    "node address is invalid".into(),
                );
                continue;
            };
            self.capabilities.insert(if node.tls {
                "upstream.https"
            } else {
                "upstream.http"
            });
            if node.backup {
                self.capabilities.insert("upstream.backup");
            }
            if node.unix_socket.is_some() {
                self.capabilities.insert("upstream.unix");
            }
            let mut endpoint = UpstreamEndpoint::new(endpoint_id(node.id), address);
            endpoint.sni.clone_from(&node.sni);
            endpoint.weight = node.weight;
            endpoint.enabled = node.enabled;
            endpoint.backup = node.backup;
            endpoint.unix_socket.clone_from(&node.unix_socket);
            endpoints.push(endpoint);
        }
        if !matches!(upstream.balancing, LoadBalancingPolicy::RoundRobin) {
            self.capabilities.insert("upstream.balancing");
        }
        if upstream.connection.http2 {
            self.capabilities.insert("upstream.http2");
        }
        if upstream.health_check.is_some() {
            self.capabilities.insert("upstream.health-check");
        }
        if upstream.passive_health.is_some() {
            self.capabilities.insert("upstream.passive-health");
        }
        let mut pool =
            UpstreamPoolSpec::new(pool_id(upstream.id), upstream.name.clone(), endpoints);
        pool.load_balancing = upstream.balancing.clone();
        pool.retry_policy = RetryPolicy::none();
        pool.connection = upstream.connection.clone();
        pool.tls = upstream.tls.clone();
        pool.host_header.clone_from(&upstream.host_header);
        pool.health_check.clone_from(&upstream.health_check);
        pool.passive_health.clone_from(&upstream.passive_health);
        self.snapshot.upstream_pools.push(pool);
    }

    fn site(&mut self, site: &Site) {
        let id = site_id(site.id);
        let domains = site
            .domains
            .iter()
            .map(|domain| {
                let mut compiled = DomainSpec::new(domain.host.clone());
                compiled.enabled = domain.enabled;
                compiled.primary = domain.primary;
                compiled.redirect_to_primary = domain.redirect;
                compiled.tls_profile_id = domain
                    .tls_profile_id
                    .clone()
                    .or_else(|| site.tls_profile_id.clone());
                compiled
            })
            .collect();
        let mut compiled = SiteSpec::new(id.clone(), site.name.clone(), domains);
        compiled.enabled = site.enabled;
        compiled.listener_ids = site.listener_ids.clone();
        compiled.https_redirect = site.https_redirect;
        compiled.www_redirect = site.www_redirect;
        if site.https_redirect
            || site.www_redirect != WwwRedirect::None
            || site.domains.iter().any(|domain| domain.redirect)
        {
            self.capabilities.insert("site.redirect");
        }
        self.snapshot.sites.push(compiled);

        for route in &site.routes {
            self.route(site, route);
        }
        let action = self.action(&site.action, &format!("{}-site", site.id));
        let mut fallback = RouteSpec::new(
            route_id(&format!("{}-site", site.id)),
            id,
            SITE_ACTION_PRIORITY,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").expect("root is a valid prefix"),
            },
            action,
        );
        fallback.name = Some("site".into());
        self.snapshot.routes.push(fallback);
    }

    fn route(&mut self, site: &Site, route: &Route) {
        let resource = format!("sites/{}/routes/{}", site.id, route.id);
        let matcher = match route.matcher.kind {
            MatchKind::Exact => {
                self.capabilities.insert("route.exact-path");
                RouteMatcher::ExactPath {
                    path: route.matcher.path.clone(),
                }
            }
            MatchKind::Glob => {
                self.capabilities.insert("route.glob");
                RouteMatcher::Glob {
                    pattern: route.matcher.path.clone(),
                }
            }
            MatchKind::Regex => {
                self.capabilities.insert("route.regex");
                RouteMatcher::Regex {
                    pattern: route.matcher.path.clone(),
                }
            }
            MatchKind::Prefix => {
                let Ok(path) = PathPrefix::new(&route.matcher.path) else {
                    self.fail(
                        resource,
                        format!("{:?} is not a path prefix", route.matcher.path),
                    );
                    return;
                };
                self.capabilities.insert("route.path-prefix");
                match &route.matcher.host {
                    Some(host) => {
                        self.capabilities.insert("route.host");
                        RouteMatcher::HostPathPrefix {
                            host: host.clone(),
                            path,
                        }
                    }
                    None => RouteMatcher::PathPrefix { path },
                }
            }
        };
        let action = self.action(&route.action, &route.id.to_string());
        let mut compiled = RouteSpec::new(
            route_id(&route.id.to_string()),
            site_id(site.id),
            route.priority,
            matcher,
            action,
        );
        compiled.enabled = route.enabled;
        compiled.name.clone_from(&route.name);
        self.snapshot.routes.push(compiled);
    }

    fn action(&mut self, action: &Action, owner: &str) -> RouteAction {
        match action {
            Action::Proxy { upstream_id } => RouteAction::Proxy {
                upstream_pool_id: pool_id(*upstream_id),
            },
            Action::Static {
                root,
                index_files,
                spa_fallback,
            } => {
                self.capabilities.insert("action.static");
                let policy_id = format!("{owner}-static");
                self.snapshot.static_content.push(StaticContentPolicy {
                    id: policy_id.clone(),
                    root: root.clone(),
                    index_files: index_files.clone(),
                    spa_fallback: *spa_fallback,
                });
                RouteAction::Static { policy_id }
            }
            Action::Redirect {
                location,
                status,
                preserve_path,
            } => {
                self.capabilities.insert("action.redirect");
                if uses_variables(location) {
                    self.capabilities.insert(TEMPLATE_CAPABILITY);
                }
                RouteAction::Redirect {
                    location: location.clone(),
                    status: *status,
                    preserve_path: *preserve_path,
                }
            }
            Action::Respond {
                status,
                body,
                content_type,
                retry_after_seconds,
            } => {
                self.capabilities.insert("action.respond");
                if body.as_deref().is_some_and(uses_variables) {
                    self.capabilities.insert(TEMPLATE_CAPABILITY);
                }
                RouteAction::Respond {
                    status: *status,
                    body: body.clone(),
                    content_type: content_type.clone(),
                    retry_after_seconds: *retry_after_seconds,
                }
            }
        }
    }
}

// UUIDs are valid IR identifiers, so these conversions cannot fail.
fn site_id(id: Uuid) -> SiteId {
    SiteId::new(id.to_string()).expect("UUIDs are valid site ids")
}

fn route_id(id: &str) -> RouteId {
    RouteId::new(id).expect("UUID-derived ids are valid route ids")
}

fn pool_id(id: Uuid) -> UpstreamPoolId {
    UpstreamPoolId::new(id.to_string()).expect("UUIDs are valid pool ids")
}

fn endpoint_id(id: Uuid) -> EndpointId {
    EndpointId::new(id.to_string()).expect("UUIDs are valid endpoint ids")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Domain, Listener, RouteMatch, Upstream, UpstreamNode};
    use chrono::Utc;
    use panel_domain::NormalizedHost;
    use panel_ir::ListenerProtocols;

    fn model() -> (ConfigModel, Uuid) {
        let upstream = Upstream {
            id: Uuid::now_v7(),
            name: "app".into(),
            nodes: vec![UpstreamNode {
                id: Uuid::now_v7(),
                host: "127.0.0.1".into(),
                port: 8080,
                tls: false,
                weight: 2,
                enabled: true,
                backup: false,
                sni: None,
                unix_socket: None,
                note: None,
            }],
            balancing: LoadBalancingPolicy::RoundRobin,
            host_header: None,
            tls: Default::default(),
            connection: Default::default(),
            health_check: None,
            passive_health: None,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let site = Site {
            id: Uuid::now_v7(),
            name: "shop".into(),
            action: Action::Proxy {
                upstream_id: upstream.id,
            },
            enabled: true,
            domains: vec![Domain {
                host: NormalizedHost::new("shop.example.com").unwrap(),
                enabled: true,
                primary: true,
                redirect: false,
                tls_profile_id: None,
            }],
            routes: vec![Route {
                id: Uuid::now_v7(),
                name: Some("assets".into()),
                enabled: true,
                priority: 10,
                matcher: RouteMatch {
                    kind: MatchKind::Prefix,
                    path: "/assets".into(),
                    host: None,
                },
                action: Action::Static {
                    root: "shop".into(),
                    index_files: vec!["index.html".into()],
                    spa_fallback: false,
                },
            }],
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            tls_profile_id: None,
            group: None,
            tags: BTreeSet::new(),
            note: None,
            favorite: false,
            deleted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let site_id = site.id;
        let model = ConfigModel {
            listeners: vec![Listener {
                id: "http".into(),
                address: "0.0.0.0:80".into(),
                tls_profile_id: None,
                protocols: ListenerProtocols::default(),
                reuse_port: false,
                ipv6_only: None,
                default_site_id: Some(site_id),
            }],
            tls_profiles: Vec::new(),
            upstreams: vec![upstream],
            sites: vec![site],
        };
        (model, site_id)
    }

    #[test]
    fn sites_become_routes_with_the_site_action_last() {
        let (model, site) = model();
        let snapshot = compile(&model, RevisionId::new(7)).unwrap();
        assert!(snapshot.has_valid_content_hash());
        assert_eq!(snapshot.revision_id, RevisionId::new(7));
        assert_eq!(snapshot.routes.len(), 2);
        let fallback = snapshot
            .routes
            .iter()
            .find(|route| route.priority == SITE_ACTION_PRIORITY)
            .unwrap();
        assert!(matches!(fallback.action, RouteAction::Proxy { .. }));
        assert_eq!(snapshot.static_content.len(), 1);
        assert_eq!(snapshot.upstream_pools[0].endpoints[0].weight, 2);
        assert_eq!(
            snapshot.listeners[0]
                .default_site_id
                .as_ref()
                .unwrap()
                .as_str(),
            site.to_string()
        );
        let required: Vec<_> = snapshot
            .required_capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect();
        assert_eq!(
            required,
            [
                "action.static",
                "listener.http",
                "listener.http2",
                "route.path-prefix",
                "upstream.http"
            ]
        );
        let report = panel_engine::validate_engine_ir(
            &snapshot,
            &required
                .iter()
                .map(|name| panel_engine::EngineCapability::new(*name, "1"))
                .collect(),
        )
        .unwrap();
        assert!(report.valid, "{:?}", report.diagnostics);
    }

    #[test]
    fn deleted_sites_and_unused_upstreams_are_left_out() {
        let (mut model, site) = model();
        model.delete_site(site, Utc::now()).unwrap();
        let snapshot = compile(&model, RevisionId::new(8)).unwrap();
        assert!(snapshot.sites.is_empty());
        assert!(snapshot.upstream_pools.is_empty());
        assert!(snapshot.listeners[0].default_site_id.is_none());
    }

    #[test]
    fn invalid_models_return_diagnostics_instead_of_snapshots() {
        let (mut model, _) = model();
        model.upstreams.clear();
        let diagnostics = compile(&model, RevisionId::new(9)).unwrap_err();
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("does not exist")));
    }
}
