//! Compiles the editable model into an engine-neutral runtime snapshot.

use crate::lua::{self, LuaConfig, LuaScope, Scripts};
use crate::model::{
    Action, ConfigModel, Favicon, MatchKind, Route, RouteCondition, Site, TlsProfile, ValueTest,
};
use panel_domain::{
    EndpointAddress, EndpointId, PathPrefix, RevisionId, RouteId, SiteId, UpstreamPoolId,
};
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::logging::LOGGING_CAPABILITY;
use panel_ir::template::{uses_variables, TEMPLATE_CAPABILITY};
use panel_ir::tls::{HSTS_CAPABILITY, TLS_SETTINGS_CAPABILITY};
use panel_ir::{
    CapabilityRequirement, DomainSpec, ListenerRef, LoadBalancingPolicy, LuaProgram, RewriteRule,
    RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec, StaticContentPolicy,
    UpstreamEndpoint, UpstreamPoolSpec, WwwRedirect, ERROR_PAGES_CAPABILITY,
    HTTP_POLICIES_CAPABILITY, LUA_SCRIPTS_CAPABILITY, MAINTENANCE_CAPABILITY, REWRITE_CAPABILITY,
};
use panel_ir::{
    REQUEST_HEAD_TIMEOUT_CAPABILITY, REQUEST_SECURITY_CAPABILITY, ROUTE_CONDITIONS_CAPABILITY,
    TRUSTED_PROXIES_CAPABILITY, UPSTREAM_RESILIENCE_CAPABILITY,
};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Required by snapshots with named locations.
pub const NAMED_ROUTE_CAPABILITY: &str = "route.named";

/// The site action runs after every route the operator defined.
const SITE_ACTION_PRIORITY: u32 = u32::MAX;

/// The paths a site may answer itself, ahead of its routes (ADR 0041).
pub(crate) const ROBOTS_PATH: &str = "/robots.txt";
pub(crate) const FAVICON_PATH: &str = "/favicon.ico";

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
        lua: &model.lua,
        scripts: Scripts::default(),
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
    if model.tls_profiles.iter().any(TlsProfile::narrows_listener) {
        compiler.capabilities.insert(TLS_SETTINGS_CAPABILITY);
    }
    for upstream in model
        .upstreams
        .iter()
        .filter(|upstream| used.contains(&upstream.id))
    {
        compiler.upstream(upstream);
    }
    let policies: BTreeSet<&str> = live
        .iter()
        .flat_map(|site| {
            site.security_policy_id.as_deref().into_iter().chain(
                site.routes
                    .iter()
                    .filter_map(|route| route.security_policy_id.as_deref()),
            )
        })
        .collect();
    compiler.snapshot.security_policies = model
        .security_policies
        .iter()
        .filter(|policy| policies.contains(policy.id.as_str()))
        .map(crate::SecurityPolicy::compile)
        .collect();
    if !policies.is_empty() {
        compiler.capabilities.insert(REQUEST_SECURITY_CAPABILITY);
    }
    let http_policies: BTreeSet<&str> = live
        .iter()
        .flat_map(|site| {
            site.http_policy_id.as_deref().into_iter().chain(
                site.routes
                    .iter()
                    .filter_map(|route| route.http_policy_id.as_deref()),
            )
        })
        .collect();
    compiler.snapshot.header_policies = model
        .http_policies
        .iter()
        .filter(|policy| http_policies.contains(policy.id.as_str()))
        .map(crate::HttpPolicy::compile)
        .collect();
    if !http_policies.is_empty() {
        compiler.capabilities.insert(HTTP_POLICIES_CAPABILITY);
    }
    for site in live {
        compiler.site(site);
    }
    compiler.snapshot.logging.clone_from(&model.logging);
    if !model.logging.is_default() {
        compiler.capabilities.insert(LOGGING_CAPABILITY);
    }
    compiler.lua_program();
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

struct Compiler<'m> {
    snapshot: RuntimeSnapshot,
    capabilities: BTreeSet<&'static str>,
    diagnostics: Vec<Diagnostic>,
    lua: &'m LuaConfig,
    scripts: Scripts,
}

impl Compiler<'_> {
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
        compiled.trusted_proxies = listener.trusted_proxies.iter().cloned().collect();
        compiled.real_ip_header = listener.real_ip_header;
        if !listener.trusted_proxies.is_empty() {
            self.capabilities.insert(TRUSTED_PROXIES_CAPABILITY);
        }
        compiled.request_head_timeout_ms = listener
            .request_head_timeout_seconds
            .map(|seconds| seconds.saturating_mul(1000));
        if compiled.request_head_timeout_ms.is_some() {
            self.capabilities.insert(REQUEST_HEAD_TIMEOUT_CAPABILITY);
        }
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
        let resilience = upstream.resilience();
        pool.retry_policy = resilience.retry_policy;
        pool.connection = upstream.connection.clone();
        pool.tls = upstream.tls.clone();
        pool.host_header.clone_from(&upstream.host_header);
        pool.health_check.clone_from(&upstream.health_check);
        pool.passive_health.clone_from(&upstream.passive_health);
        pool.circuit_breaker = resilience.circuit_breaker;
        pool.max_requests = resilience.max_requests;
        pool.queue = resilience.queue;
        pool.balancer = upstream.balancer.as_ref().map(|code| {
            let id = self.scripts.add(code, &self.lua.files);
            lua::handler(id, &self.lua.http)
        });
        if panel_engine::uses_resilience(&pool) {
            self.capabilities.insert(UPSTREAM_RESILIENCE_CAPABILITY);
        }
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
        compiled.hsts = site.hsts;
        compiled
            .security_policy_id
            .clone_from(&site.security_policy_id);
        compiled.header_policy_id.clone_from(&site.http_policy_id);
        compiled.access_log.clone_from(&site.access_log);
        if !site.access_log.is_unset() {
            self.capabilities.insert(LOGGING_CAPABILITY);
        }
        compiled.rewrites.clone_from(&site.rewrites);
        if !site.rewrites.is_empty() {
            self.capabilities.insert(REWRITE_CAPABILITY);
        }
        compiled.error_pages.clone_from(&site.error_pages);
        if !site.error_pages.is_empty() {
            self.capabilities.insert(ERROR_PAGES_CAPABILITY);
        }
        compiled.maintenance = site
            .maintenance
            .as_ref()
            .filter(|maintenance| maintenance.enabled)
            .map(crate::SiteMaintenance::runtime);
        if compiled.maintenance.is_some() {
            self.capabilities.insert(MAINTENANCE_CAPABILITY);
        }
        if site.hsts.is_some() {
            self.capabilities.insert(HSTS_CAPABILITY);
        }
        if site.https_redirect
            || site.www_redirect != WwwRedirect::None
            || site.domains.iter().any(|domain| domain.redirect)
        {
            self.capabilities.insert("site.redirect");
        }
        let scope = site.lua.over(&self.lua.http);
        compiled.lua = lua::handlers(&scope, &mut self.scripts, &self.lua.files, false);
        compiled.lua.variables = lua::variables(
            self.lua.http.variables.iter().chain(&site.lua.variables),
            &scope,
            &mut self.scripts,
            &self.lua.files,
        );
        self.snapshot.sites.push(compiled);

        self.site_files(site, &scope);
        for route in &site.routes {
            self.route(site, route, &scope);
        }
        let action = self.action(&site.action, &format!("{}-site", site.id), &scope);
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
        fallback.lua = lua::handlers(&scope, &mut self.scripts, &self.lua.files, true);
        self.snapshot.routes.push(fallback);
    }

    /// The exact-path routes answering `/robots.txt` and `/favicon.ico` for
    /// a site that answers them itself, ahead of every route of its own.
    fn site_files(&mut self, site: &Site, scope: &LuaScope) {
        let mut answered = Vec::new();
        if let Some(robots) = &site.robots {
            self.capabilities.insert("action.respond");
            let action = RouteAction::Respond {
                status: 200,
                body: Some(literal(robots.body())),
                content_type: Some("text/plain; charset=utf-8".into()),
                retry_after_seconds: None,
            };
            answered.push(("robots", ROBOTS_PATH, action, Vec::new()));
        }
        if let Some(favicon) = &site.favicon {
            let (action, rewrites) = match favicon {
                Favicon::NoContent => {
                    self.capabilities.insert("action.respond");
                    let action = RouteAction::Respond {
                        status: 204,
                        body: None,
                        content_type: None,
                        retry_after_seconds: None,
                    };
                    (action, Vec::new())
                }
                Favicon::File { path } => {
                    let (directory, name) = path
                        .rsplit_once('/')
                        .expect("validation keeps favicon files in a directory");
                    self.capabilities.insert("action.static");
                    self.capabilities.insert(REWRITE_CAPABILITY);
                    let policy_id = format!("{}-favicon-static", site.id);
                    self.snapshot.static_content.push(StaticContentPolicy {
                        id: policy_id.clone(),
                        root: directory.to_owned(),
                        index_files: Vec::new(),
                        spa_fallback: false,
                    });
                    let rewrite = RewriteRule::SetUri {
                        template: literal(&format!("/{name}")),
                    };
                    (RouteAction::Static { policy_id }, vec![rewrite])
                }
                Favicon::Redirect { location } => {
                    self.capabilities.insert("action.redirect");
                    let action = RouteAction::Redirect {
                        location: literal(location),
                        status: 302,
                        preserve_path: false,
                    };
                    (action, Vec::new())
                }
            };
            answered.push(("favicon", FAVICON_PATH, action, rewrites));
        }
        for (name, path, action, rewrites) in answered {
            self.capabilities.insert("route.exact-path");
            let mut route = RouteSpec::new(
                route_id(&format!("{}-{name}", site.id)),
                site_id(site.id),
                0,
                RouteMatcher::ExactPath { path: path.into() },
                action,
            );
            route.name = Some(name.into());
            route.rewrites = rewrites;
            route.lua = lua::handlers(scope, &mut self.scripts, &self.lua.files, true);
            self.snapshot.routes.push(route);
        }
    }

    fn route(&mut self, site: &Site, route: &Route, site_scope: &LuaScope) {
        let resource = format!("sites/{}/routes/{}", site.id, route.id);
        let matcher = match (&route.named, route.matcher.kind) {
            (Some(name), _) => {
                self.capabilities.insert(NAMED_ROUTE_CAPABILITY);
                RouteMatcher::Named { name: name.clone() }
            }
            (None, MatchKind::Exact) => {
                self.capabilities.insert("route.exact-path");
                RouteMatcher::ExactPath {
                    path: route.matcher.path.clone(),
                }
            }
            (None, MatchKind::Glob) => {
                self.capabilities.insert("route.glob");
                RouteMatcher::Glob {
                    pattern: route.matcher.path.clone(),
                }
            }
            (None, MatchKind::Regex) => {
                self.capabilities.insert("route.regex");
                RouteMatcher::Regex {
                    pattern: route.matcher.path.clone(),
                }
            }
            (None, MatchKind::Prefix) => {
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
        let scope = route.lua.over(site_scope);
        let action = self.action(&route.action, &route.id.to_string(), &scope);
        let mut compiled = RouteSpec::new(
            route_id(&route.id.to_string()),
            site_id(site.id),
            route.priority,
            matcher,
            action,
        );
        compiled.enabled = route.enabled;
        compiled.conditions = conditions(&route.matcher.conditions);
        if !compiled.conditions.is_empty() {
            self.capabilities.insert(ROUTE_CONDITIONS_CAPABILITY);
        }
        compiled.name.clone_from(&route.name);
        compiled
            .security_policy_id
            .clone_from(&route.security_policy_id);
        compiled.header_policy_id.clone_from(&route.http_policy_id);
        compiled.access_log.clone_from(&route.access_log);
        if !route.access_log.is_unset() {
            self.capabilities.insert(LOGGING_CAPABILITY);
        }
        compiled.rewrites.clone_from(&route.rewrites);
        compiled.internal = route.internal;
        if !route.rewrites.is_empty() || route.internal {
            self.capabilities.insert(REWRITE_CAPABILITY);
        }
        compiled.error_pages.clone_from(&route.error_pages);
        if route
            .error_pages
            .as_ref()
            .is_some_and(|pages| !pages.is_empty())
        {
            self.capabilities.insert(ERROR_PAGES_CAPABILITY);
        }
        compiled.lua = lua::handlers(&scope, &mut self.scripts, &self.lua.files, true);
        compiled.lua.variables = lua::variables(
            &route.lua.variables,
            &scope,
            &mut self.scripts,
            &self.lua.files,
        );
        self.snapshot.routes.push(compiled);
    }

    /// The scripts the handlers use and the modules they may load, once
    /// any handler is compiled.
    fn lua_program(&mut self) {
        let config = self.lua;
        let mut program = LuaProgram {
            disabled: config.disabled,
            init: config
                .init
                .as_ref()
                .map(|code| lua::handler(self.scripts.add(code, &config.files), &config.http)),
            init_worker: config
                .init_worker
                .as_ref()
                .map(|code| lua::handler(self.scripts.add(code, &config.files), &config.http)),
            exit_worker: config
                .exit_worker
                .as_ref()
                .map(|code| lua::handler(self.scripts.add(code, &config.files), &config.http)),
            ssl_session_fetch: config
                .ssl_session_fetch
                .as_ref()
                .map(|code| lua::handler(self.scripts.add(code, &config.files), &config.http)),
            ssl_session_store: config
                .ssl_session_store
                .as_ref()
                .map(|code| lua::handler(self.scripts.add(code, &config.files), &config.http)),
            ..LuaProgram::default()
        };
        if self.scripts.is_empty() {
            let variables = self
                .snapshot
                .sites
                .iter()
                .map(|site| &site.lua)
                .chain(self.snapshot.routes.iter().map(|route| &route.lua))
                .any(|lua| !lua.variables.is_empty());
            if variables {
                self.capabilities.insert(LUA_SCRIPTS_CAPABILITY);
            }
            return;
        }
        self.scripts.modules(&config.files);
        program.scripts = std::mem::take(&mut self.scripts).into_scripts();
        program.shared_dicts.clone_from(&config.shared_dicts);
        program.memory_limit_bytes = config.memory_limit_bytes.unwrap_or(0);
        program.max_pending_timers = config.max_pending_timers.unwrap_or(0);
        program.max_running_timers = config.max_running_timers.unwrap_or(0);
        program.regex_cache_max_entries = config.regex_cache_max_entries;
        program.regex_match_limit = config.regex_match_limit.unwrap_or(0);
        program.access_first = config.access_no_postpone.unwrap_or(false);
        program.worker_thread_vm_pool_size = config.worker_thread_vm_pool_size.unwrap_or(0);
        program.capture_error_log_bytes = config.capture_error_log_bytes.unwrap_or(0);
        self.snapshot.lua = program;
        self.capabilities.insert(LUA_SCRIPTS_CAPABILITY);
    }

    fn action(&mut self, action: &Action, owner: &str, scope: &LuaScope) -> RouteAction {
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
            Action::Lua { code } => RouteAction::Lua {
                handler: lua::handler(self.scripts.add(code, &self.lua.files), scope),
            },
            Action::InternalRedirect { target } => {
                self.capabilities.insert(REWRITE_CAPABILITY);
                RouteAction::InternalRedirect {
                    target: target.clone(),
                }
            }
        }
    }
}

/// `text` as a template without variables.
fn literal(text: &str) -> String {
    text.replace('$', "$$")
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

/// A route's conditions as the IR carries them.
pub(crate) fn conditions(conditions: &[RouteCondition]) -> Vec<panel_ir::RouteCondition> {
    conditions.iter().map(condition).collect()
}

fn condition(value: &RouteCondition) -> panel_ir::RouteCondition {
    use panel_ir::RouteCondition as Ir;
    match value {
        RouteCondition::Method { methods } => Ir::Method {
            methods: methods.clone(),
        },
        RouteCondition::Host { hosts } => Ir::Host {
            hosts: hosts.clone(),
        },
        RouteCondition::Header { name, test } => Ir::Header {
            name: name.clone(),
            test: value_test(test),
        },
        RouteCondition::Query { name, test } => Ir::Query {
            name: name.clone(),
            test: value_test(test),
        },
        RouteCondition::Cookie { name, test } => Ir::Cookie {
            name: name.clone(),
            test: value_test(test),
        },
        RouteCondition::Client { networks } => Ir::Client {
            networks: networks.clone(),
        },
        RouteCondition::UserAgent { test } => Ir::UserAgent {
            test: value_test(test),
        },
        RouteCondition::Referer { test } => Ir::Referer {
            test: value_test(test),
        },
        RouteCondition::ContentType { types } => Ir::ContentType {
            types: types.clone(),
        },
        RouteCondition::All { conditions: all } => Ir::All {
            conditions: conditions(all),
        },
        RouteCondition::Any { conditions: any } => Ir::Any {
            conditions: conditions(any),
        },
        RouteCondition::Not { condition: inner } => Ir::Not {
            condition: Box::new(condition(inner)),
        },
    }
}

fn value_test(test: &ValueTest) -> panel_ir::ValueTest {
    use panel_ir::ValueTest as Ir;
    match test.clone() {
        ValueTest::Present => Ir::Present,
        ValueTest::Absent => Ir::Absent,
        ValueTest::Equals { value, ignore_case } => Ir::Equals { value, ignore_case },
        ValueTest::Prefix { value, ignore_case } => Ir::Prefix { value, ignore_case },
        ValueTest::Suffix { value, ignore_case } => Ir::Suffix { value, ignore_case },
        ValueTest::Contains { value, ignore_case } => Ir::Contains { value, ignore_case },
        ValueTest::Regex {
            pattern,
            ignore_case,
        } => Ir::Regex {
            pattern,
            ignore_case,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Domain, Listener, RouteMatch, Upstream, UpstreamNode};
    use chrono::Utc;
    use panel_domain::NormalizedHost;
    use panel_ir::{ListenerProtocols, StrictTransportSecurity};

    fn model() -> (ConfigModel, Uuid) {
        let upstream = Upstream {
            balancer: Default::default(),
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
            retry: None,
            circuit_breaker: None,
            max_requests: None,
            queue: None,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let site = Site {
            error_pages: Default::default(),
            maintenance: None,
            robots: None,
            favicon: None,
            lua: Default::default(),
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
                error_pages: None,
                lua: Default::default(),
                named: None,
                id: Uuid::now_v7(),
                name: Some("assets".into()),
                enabled: true,
                priority: 10,
                matcher: RouteMatch {
                    kind: MatchKind::Prefix,
                    path: "/assets".into(),
                    host: None,
                    conditions: Vec::new(),
                },
                action: Action::Static {
                    root: "shop".into(),
                    index_files: vec!["index.html".into()],
                    spa_fallback: false,
                },
                security_policy_id: Default::default(),
                http_policy_id: None,
                access_log: Default::default(),
                rewrites: Vec::new(),
                internal: false,
            }],
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            tls_profile_id: None,
            hsts: None,
            group: None,
            tags: BTreeSet::new(),
            note: None,
            favorite: false,
            deleted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_policy_id: Default::default(),
            http_policy_id: None,
            access_log: Default::default(),
            rewrites: Vec::new(),
        };
        let site_id = site.id;
        let model = ConfigModel {
            lua: Default::default(),
            listeners: vec![Listener {
                id: "http".into(),
                address: "0.0.0.0:80".into(),
                tls_profile_id: None,
                protocols: ListenerProtocols::default(),
                reuse_port: false,
                ipv6_only: None,
                default_site_id: Some(site_id),
                real_ip_header: Default::default(),
                trusted_proxies: Default::default(),
                request_head_timeout_seconds: Default::default(),
            }],
            tls_profiles: Vec::new(),
            upstreams: vec![upstream],
            sites: vec![site],
            security_policies: Default::default(),
            http_policies: Default::default(),
            logging: Default::default(),
        };
        (model, site_id)
    }

    #[test]
    fn logging_settings_reach_the_snapshot_and_require_their_capability() {
        let (mut model, _) = model();
        let plain = compile(&model, RevisionId::new(1)).unwrap();
        assert!(!plain
            .required_capabilities
            .iter()
            .any(|capability| capability.name == LOGGING_CAPABILITY));

        model.logging.files.keep_days = 30;
        model.sites[0].access_log.enabled = Some(false);
        model.sites[0].routes[0].access_log.format = Some(panel_ir::AccessLogFormat::Combined);
        let snapshot = compile(&model, RevisionId::new(2)).unwrap();
        assert_eq!(snapshot.logging.files.keep_days, 30);
        assert_eq!(snapshot.sites[0].access_log.enabled, Some(false));
        assert!(snapshot
            .routes
            .iter()
            .any(|route| route.access_log.format == Some(panel_ir::AccessLogFormat::Combined)));
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == LOGGING_CAPABILITY));
        assert!(snapshot.has_valid_content_hash());
    }

    #[test]
    fn lua_handlers_are_inherited_from_http_through_sites_to_routes() {
        use crate::lua::{LuaCode, LuaPermissions, LuaScope, LuaSharedDict, LuaVariable};
        let (mut model, _) = model();
        let unused = plain_route_count(&compile(&model, RevisionId::new(1)).unwrap());
        assert!(unused > 0);
        model.lua.files.insert(
            "lua/auth.lua".into(),
            "return { check = function() end }".into(),
        );
        model
            .lua
            .files
            .insert("lua/pick/init.lua".into(), "return 1".into());
        model.lua.init = Some(LuaCode::inline("cache = {}"));
        model.lua.shared_dicts.push(LuaSharedDict {
            name: "hits".into(),
            capacity_bytes: 1 << 20,
        });
        model.lua.http = LuaScope {
            access: Some(LuaCode::Inline {
                code: "require('auth').check()".into(),
                file: Some("main.conf".into()),
                line: 4,
            }),
            log: Some(LuaCode::inline("local n = 1")),
            time_limit_ms: Some(50),
            allow: Some(LuaPermissions {
                upstream: true,
                ..LuaPermissions::default()
            }),
            variables: vec![LuaVariable::value("base", "b")],
            ..LuaScope::default()
        };
        model.sites[0].lua = LuaScope {
            server_rewrite: Some(LuaCode::inline("ngx.req.set_uri('/x')")),
            time_limit_ms: Some(20),
            variables: vec![LuaVariable::script(
                "tenant",
                LuaCode::inline("return ngx.arg[1]"),
                vec!["$http_x_tenant".into()],
            )],
            ..LuaScope::default()
        };
        model.sites[0].routes[0].lua = LuaScope {
            access: Some(LuaCode::file("lua/auth.lua")),
            debug: Some(true),
            variables: vec![LuaVariable::value("r", "1")],
            ..LuaScope::default()
        };
        model.sites[0].routes[0].action = Action::Lua {
            code: LuaCode::inline("ngx.say('hi')"),
        };
        model.upstreams[0].balancer = Some(LuaCode::file("lua/pick/init.lua"));
        let snapshot = compile(&model, RevisionId::new(2)).unwrap();

        let site = &snapshot.sites[0].lua;
        assert_eq!(site.server_rewrite.as_ref().unwrap().time_limit_ms, 20);
        let names: Vec<_> = site.variables.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["base", "tenant"]);
        assert_eq!(site.variables[0].value, "b");
        let tenant = &site.variables[1];
        assert_eq!(tenant.handler.as_ref().unwrap().time_limit_ms, 20);
        assert_eq!(tenant.args, ["$http_x_tenant"]);
        assert_eq!(site.access.as_ref().unwrap().script_id, "main.conf:4");
        let route = snapshot
            .routes
            .iter()
            .find(|route| route.name.as_deref() == Some("assets"))
            .unwrap();
        let access = route.lua.access.as_ref().unwrap();
        assert_eq!(access.script_id, "lua/auth.lua");
        assert_eq!(access.time_limit_ms, 20);
        assert!(access.debug && access.allow.upstream);
        assert!(route.lua.server_rewrite.is_none());
        assert!(route.lua.log.is_some());
        assert_eq!(route.lua.variables.len(), 1);
        assert_eq!(route.lua.variables[0].name, "r");
        let RouteAction::Lua { handler } = &route.action else {
            panic!("the route answers with Lua");
        };
        assert!(handler.debug);
        let fallback = snapshot
            .routes
            .iter()
            .find(|route| route.name.as_deref() == Some("site"))
            .unwrap();
        assert_eq!(
            fallback.lua.access.as_ref().unwrap().script_id,
            "main.conf:4"
        );
        assert!(fallback.lua.server_rewrite.is_none());
        let balancer = snapshot.upstream_pools[0].balancer.as_ref().unwrap();
        assert_eq!(balancer.script_id, "lua/pick/init.lua");
        assert_eq!(balancer.time_limit_ms, 50);

        let program = &snapshot.lua;
        assert!(program.init.is_some());
        assert_eq!(program.shared_dicts.len(), 1);
        let modules: Vec<_> = program
            .scripts
            .iter()
            .filter_map(|script| script.module.as_deref())
            .collect();
        assert_eq!(modules, ["auth", "pick"]);
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == LUA_SCRIPTS_CAPABILITY));
        assert_eq!(panel_engine::lua_problems(&snapshot), Vec::new());
        assert!(snapshot.has_valid_content_hash());
    }

    fn plain_route_count(snapshot: &RuntimeSnapshot) -> usize {
        assert!(snapshot.lua.is_empty());
        assert!(!snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == LUA_SCRIPTS_CAPABILITY));
        snapshot.routes.len()
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
    fn tls_settings_and_hsts_require_their_capabilities() {
        let (mut model, _) = model();
        model.tls_profiles.push(crate::TlsProfile {
            id: "edge".into(),
            certificate_id: Some(panel_domain::CertificateId::new("example.com").unwrap()),
            certificate_secret_id: String::new(),
            private_key_secret_id: String::new(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: Some("TLSv1.2".into()),
            cipher_suites: Vec::new(),
            session_resumption: true,
            ocsp_stapling: false,
            alpn: BTreeSet::new(),
        });
        model.sites[0].hsts = Some(StrictTransportSecurity {
            max_age_seconds: 63_072_000,
            include_subdomains: true,
            preload: true,
        });
        let snapshot = compile(&model, RevisionId::new(9)).unwrap();
        let required: Vec<_> = snapshot
            .required_capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect();
        assert!(required.contains(&TLS_SETTINGS_CAPABILITY), "{required:?}");
        assert!(required.contains(&HSTS_CAPABILITY), "{required:?}");
        let profile = &snapshot.tls_profiles[0];
        assert_eq!(profile.max_protocol.as_deref(), Some("TLSv1.2"));
        assert_eq!(profile.certificate_secret_id, "cert-example.com.pem");
        assert_eq!(
            snapshot.sites[0].hsts.unwrap().header_value(),
            "max-age=63072000; includeSubDomains; preload"
        );
    }

    #[test]
    fn upstream_resilience_reaches_the_snapshot_and_requires_its_capability() {
        let (mut model, _) = model();
        let snapshot = compile(&model, RevisionId::new(9)).unwrap();
        assert!(!snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == UPSTREAM_RESILIENCE_CAPABILITY));
        model.upstreams[0].retry = Some(crate::UpstreamRetry {
            attempts: 2,
            statuses: [503].into(),
            on: [panel_ir::RetryCondition::Reset].into(),
            ..crate::UpstreamRetry::default()
        });
        model.upstreams[0].circuit_breaker = Some(panel_ir::CircuitBreaker {
            failure_percent: 50,
            min_requests: 20,
            open_ms: 30_000,
            half_open_requests: 1,
        });
        model.upstreams[0].max_requests = Some(10);
        model.upstreams[0].queue = Some(panel_ir::UpstreamQueue {
            max_waiting: 5,
            timeout_ms: 1_000,
        });
        let snapshot = compile(&model, RevisionId::new(10)).unwrap();
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == UPSTREAM_RESILIENCE_CAPABILITY));
        let pool = &snapshot.upstream_pools[0];
        assert_eq!(pool.retry_policy.attempts, 2);
        assert!(pool.retry_policy.retry_statuses.contains(&503));
        assert_eq!(pool.max_requests, Some(10));
        assert_eq!(pool.queue.unwrap().max_waiting, 5);
        assert!(pool.circuit_breaker.is_some());

        model.upstreams[0].max_requests = None;
        let messages: Vec<String> = crate::validate(&model)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert!(
            messages.iter().any(|message| message
                == "the upstream queues requests without a limit of requests at once"),
            "{messages:?}"
        );
    }

    #[test]
    fn http_policies_reach_the_snapshot_through_their_users() {
        let (mut model, _) = model();
        let field = |name: &str, value: &str| panel_ir::HeaderField {
            name: name.into(),
            value: value.into(),
        };
        assert!(model.put_http_policy(crate::HttpPolicy {
            id: "site".into(),
            response: crate::FieldChanges {
                set: vec![field("X-Frame-Options", "DENY")],
                ..crate::FieldChanges::default()
            },
            server: panel_ir::ServerHeader::Remove,
            ..crate::HttpPolicy::default()
        }));
        assert!(model.put_http_policy(crate::HttpPolicy {
            id: "api".into(),
            request: crate::FieldChanges {
                set: vec![field("X-Tenant", "$host")],
                ..crate::FieldChanges::default()
            },
            ..crate::HttpPolicy::default()
        }));
        assert!(model.put_http_policy(crate::HttpPolicy {
            id: "unused".into(),
            ..crate::HttpPolicy::default()
        }));
        model.sites[0].http_policy_id = Some("site".into());
        model.sites[0].routes[0].http_policy_id = Some("api".into());

        let snapshot = compile(&model, RevisionId::new(10)).unwrap();
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == HTTP_POLICIES_CAPABILITY));
        let ids: Vec<_> = snapshot
            .header_policies
            .iter()
            .map(|policy| policy.id.as_str())
            .collect();
        assert_eq!(ids, ["site", "api"]);
        assert_eq!(
            snapshot.header_policies[0].response_set["x-frame-options"],
            "DENY"
        );
        assert_eq!(snapshot.sites[0].header_policy_id.as_deref(), Some("site"));
        assert_eq!(snapshot.routes[0].header_policy_id.as_deref(), Some("api"));
        assert!(model.clone().delete_http_policy("api").is_err());
        assert!(model.clone().delete_http_policy("unused").is_ok());

        model.sites[0].routes[0].http_policy_id = Some("missing".into());
        model.http_policies[1].request.set.push(field("Host", "x"));
        let messages: Vec<String> = crate::validate(&model)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        for expected in [
            "HTTP policy missing does not exist",
            "the policy changes the request field host, which the gateway keeps",
        ] {
            assert!(
                messages.iter().any(|message| message == expected),
                "{expected}: {messages:?}"
            );
        }
    }

    #[test]
    fn security_policies_reach_the_snapshot_through_their_users() {
        let (mut model, site) = model();
        let unused = crate::SecurityPolicy {
            id: "unused".into(),
            ..crate::SecurityPolicy::default()
        };
        assert!(model.put_security_policy(crate::SecurityPolicy {
            id: "staff".into(),
            allowed_methods: vec!["get".into()],
            body_timeout_seconds: Some(5),
            ..crate::SecurityPolicy::default()
        }));
        assert!(model.put_security_policy(unused));
        model.sites[0].security_policy_id = Some("staff".into());
        model.sites[0].routes[0].security_policy_id = Some("staff".into());
        model.listeners[0].trusted_proxies = vec!["10.0.0.0/8".into()];
        model.listeners[0].request_head_timeout_seconds = Some(15);

        let snapshot = compile(&model, RevisionId::new(10)).unwrap();
        let required: Vec<_> = snapshot
            .required_capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect();
        assert!(
            required.contains(&REQUEST_SECURITY_CAPABILITY),
            "{required:?}"
        );
        assert!(
            required.contains(&TRUSTED_PROXIES_CAPABILITY),
            "{required:?}"
        );
        assert_eq!(snapshot.security_policies.len(), 1);
        let policy = &snapshot.security_policies[0];
        assert!(policy.allowed_methods.contains("GET"));
        assert_eq!(policy.body_timeout_ms, Some(5000));
        assert_eq!(
            snapshot.sites[0].security_policy_id.as_deref(),
            Some("staff")
        );
        assert_eq!(
            snapshot.routes[0].security_policy_id.as_deref(),
            Some("staff")
        );
        assert!(snapshot.listeners[0].trusted_proxies.contains("10.0.0.0/8"));
        assert_eq!(snapshot.listeners[0].request_head_timeout_ms, Some(15_000));
        assert!(
            required.contains(&REQUEST_HEAD_TIMEOUT_CAPABILITY),
            "{required:?}"
        );
        assert!(model.clone().delete_security_policy("staff").is_err());
        assert!(model.clone().delete_security_policy("unused").is_ok());

        model.sites[0].routes[0].security_policy_id = Some("missing".into());
        model.listeners[0].trusted_proxies = vec!["proxy".into()];
        model.listeners[0].request_head_timeout_seconds = Some(0);
        let messages: Vec<String> = crate::validate(&model)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message == "security policy missing does not exist"),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("trusted proxy")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("request head timeout")),
            "{messages:?}"
        );
        model.sites[0].routes[0].security_policy_id = None;
        model.listeners[0].trusted_proxies.clear();
        model.listeners[0].request_head_timeout_seconds = None;
        model.delete_site(site, Utc::now()).unwrap();
        let snapshot = compile(&model, RevisionId::new(11)).unwrap();
        assert!(snapshot.security_policies.is_empty());
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
    fn route_conditions_reach_the_snapshot_and_require_their_capability() {
        let (mut model, _) = model();
        model.sites[0].routes[0].matcher.conditions = vec![
            RouteCondition::Method {
                methods: vec!["GET".into()],
            },
            RouteCondition::Not {
                condition: Box::new(RouteCondition::Client {
                    networks: vec!["192.0.2.0/24".into()],
                }),
            },
        ];
        let snapshot = compile(&model, RevisionId::new(3)).unwrap();
        let route = snapshot
            .routes
            .iter()
            .find(|route| !route.conditions.is_empty())
            .unwrap();
        assert_eq!(
            route.conditions[0],
            panel_ir::RouteCondition::Method {
                methods: vec!["GET".into()]
            }
        );
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == ROUTE_CONDITIONS_CAPABILITY));

        model.sites[0].routes[0].matcher.conditions = vec![RouteCondition::Client {
            networks: vec!["10.0.0.0/33".into()],
        }];
        let diagnostics = compile(&model, RevisionId::new(4)).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("is not a network")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn rewrites_reach_the_snapshot_and_require_their_capability() {
        use panel_ir::{RewriteFlag, RewriteRule};

        let (mut model, _) = model();
        model.sites[0].rewrites = vec![RewriteRule::Rewrite {
            pattern: "^/old/(.*)$".into(),
            replacement: "/new/$1".into(),
            flag: RewriteFlag::Permanent,
        }];
        let route = &mut model.sites[0].routes[0];
        route.rewrites = vec![RewriteRule::StripPrefix {
            prefix: "/assets".into(),
        }];
        route.internal = true;
        let mut fallback = route.clone();
        fallback.id = Uuid::now_v7();
        fallback.named = Some("fallback".into());
        fallback.rewrites.clear();
        fallback.internal = false;
        let mut redirect = fallback.clone();
        redirect.id = Uuid::now_v7();
        redirect.named = None;
        redirect.matcher.path = "/gone".into();
        redirect.action = Action::InternalRedirect {
            target: "@fallback".into(),
        };
        model.sites[0].routes.extend([fallback, redirect]);
        let snapshot = compile(&model, RevisionId::new(5)).unwrap();
        assert_eq!(snapshot.sites[0].rewrites, model.sites[0].rewrites);
        let assets = snapshot.routes.iter().find(|route| route.internal).unwrap();
        assert_eq!(assets.rewrites, model.sites[0].routes[0].rewrites);
        assert!(snapshot.routes.iter().any(|route| route.action
            == RouteAction::InternalRedirect {
                target: "@fallback".into()
            }));
        assert!(snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == REWRITE_CAPABILITY));

        model.sites[0].routes[2].action = Action::InternalRedirect {
            target: "@nowhere".into(),
        };
        model.sites[0].rewrites = vec![RewriteRule::AddPrefix {
            prefix: "/v2/".into(),
        }];
        let diagnostics = compile(&model, RevisionId::new(6)).unwrap_err();
        let messages: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("@nowhere")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("the site has a rewrite prefix")),
            "{messages:?}"
        );

        let (mut model, _) = self::model();
        model.sites[0].action = Action::InternalRedirect {
            target: "/elsewhere".into(),
        };
        let diagnostics = compile(&model, RevisionId::new(7)).unwrap_err();
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("does not redirect internally")));
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

    #[test]
    fn error_pages_maintenance_and_site_files_compile_to_what_gateways_know() {
        use crate::model::{Favicon, Robots, SiteMaintenance};
        use panel_ir::{ErrorPage, ErrorPages, ErrorResponse};

        let (mut model, _) = model();
        let pages = ErrorPages {
            pages: vec![ErrorPage {
                statuses: [404, 503].into(),
                response: ErrorResponse::Body {
                    body: "<h1>$host is resting</h1>".into(),
                    content_type: None,
                },
                status: None,
            }],
            intercept: true,
        };
        let site = &mut model.sites[0];
        site.error_pages = pages.clone();
        site.maintenance = Some(SiteMaintenance {
            enabled: true,
            status: 503,
            body: None,
            content_type: None,
            retry_after_seconds: Some(600),
            allow: vec!["10.0.0.0/8".into()],
        });
        site.robots = Some(Robots::Custom {
            body: "User-agent: *\nDisallow: /*.php$\n".into(),
        });
        site.favicon = Some(Favicon::File {
            path: "shop/brand/favicon.png".into(),
        });
        site.routes[0].error_pages = Some(ErrorPages::default());
        let snapshot = compile(&model, RevisionId::new(8)).unwrap();
        assert_eq!(snapshot.sites[0].error_pages, pages);
        let maintenance = snapshot.sites[0].maintenance.as_ref().unwrap();
        assert_eq!(maintenance.retry_after_seconds, Some(600));
        assert_eq!(maintenance.allow, ["10.0.0.0/8"]);
        let named = |name: &str| {
            snapshot
                .routes
                .iter()
                .find(|route| route.name.as_deref() == Some(name))
                .unwrap()
        };
        let robots = named("robots");
        assert_eq!(
            (&robots.matcher, robots.priority),
            (
                &RouteMatcher::ExactPath {
                    path: "/robots.txt".into()
                },
                0
            )
        );
        assert_eq!(
            robots.action,
            RouteAction::Respond {
                status: 200,
                body: Some("User-agent: *\nDisallow: /*.php$$\n".into()),
                content_type: Some("text/plain; charset=utf-8".into()),
                retry_after_seconds: None,
            }
        );
        let favicon = named("favicon");
        assert_eq!(
            favicon.rewrites,
            [RewriteRule::SetUri {
                template: "/favicon.png".into()
            }]
        );
        let RouteAction::Static { policy_id } = &favicon.action else {
            panic!("{:?}", favicon.action);
        };
        let policy = snapshot
            .static_content
            .iter()
            .find(|policy| &policy.id == policy_id)
            .unwrap();
        assert_eq!(policy.root, "shop/brand");
        assert_eq!(named("assets").error_pages, Some(ErrorPages::default()));
        let required: Vec<_> = snapshot
            .required_capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect();
        for capability in [
            ERROR_PAGES_CAPABILITY,
            MAINTENANCE_CAPABILITY,
            REWRITE_CAPABILITY,
            "route.exact-path",
            "action.respond",
        ] {
            assert!(
                required.contains(&capability),
                "{capability} in {required:?}"
            );
        }
        let report = panel_engine::validate_engine_ir(
            &snapshot,
            &required
                .iter()
                .map(|name| panel_engine::EngineCapability::new(*name, "1"))
                .collect(),
        )
        .unwrap();
        assert!(report.valid, "{:?}", report.diagnostics);

        let site = &mut model.sites[0];
        site.maintenance.as_mut().unwrap().enabled = false;
        site.favicon = Some(Favicon::Redirect {
            location: "https://cdn.example.com/icon.png".into(),
        });
        let snapshot = compile(&model, RevisionId::new(9)).unwrap();
        assert!(snapshot.sites[0].maintenance.is_none());
        assert!(!snapshot
            .required_capabilities
            .iter()
            .any(|capability| capability.name == MAINTENANCE_CAPABILITY));
        let favicon = snapshot
            .routes
            .iter()
            .find(|route| route.name.as_deref() == Some("favicon"))
            .unwrap();
        assert_eq!(
            favicon.action,
            RouteAction::Redirect {
                location: "https://cdn.example.com/icon.png".into(),
                status: 302,
                preserve_path: false,
            }
        );
    }
}
