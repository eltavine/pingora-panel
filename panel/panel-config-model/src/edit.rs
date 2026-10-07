//! Mutations that keep the model's invariants: identities and timestamps are
//! assigned here, and a change that introduces a validation error is refused
//! as a whole.

use crate::{
    cache::CachePolicy,
    http::HttpPolicy,
    model::{
        Action, ConfigModel, Domain, Favicon, Listener, Robots, Route, RouteMatch, Site,
        SiteMaintenance, TlsProfile, Upstream, UpstreamNode, UpstreamRetry,
    },
    security::SecurityPolicy,
    validate::validate,
    MODEL_VERSION,
};
use chrono::{DateTime, Utc};
use panel_domain::NormalizedHost;
use panel_errors::{Diagnostic, PanelError, Result};
use panel_ir::{
    AccessLog, ActiveHealthCheck, CircuitBreaker, ErrorPages, LoadBalancingPolicy,
    PassiveHealthPolicy, RewriteRule, StrictTransportSecurity, UpstreamConnectionPolicy,
    UpstreamQueue, UpstreamTlsPolicy, WwwRedirect,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

const fn enabled() -> bool {
    true
}

const fn one() -> u32 {
    1
}

/// Reads a field that may be `null`, so leaving it out and clearing it are
/// told apart.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A site as clients write it; identity and timestamps are server-managed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SiteInput {
    pub name: String,
    pub action: Action,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub domains: Vec<Domain>,
    #[serde(default)]
    pub routes: Vec<RouteInput>,
    #[serde(default)]
    pub listener_ids: BTreeSet<String>,
    #[serde(default)]
    pub https_redirect: bool,
    #[serde(default)]
    pub www_redirect: WwwRedirect,
    #[serde(default)]
    pub tls_profile_id: Option<String>,
    /// Sent with HTTPS responses for the site's hosts.
    #[serde(default)]
    pub hsts: Option<StrictTransportSecurity>,
    /// Restrictions every request to the site passes first.
    #[serde(default)]
    pub security_policy_id: Option<String>,
    /// Field changes, CORS and compression for the site's requests.
    #[serde(default)]
    pub http_policy_id: Option<String>,
    /// What caches the site's responses; `null` caches none and absent keeps
    /// the current policy.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub cache_policy_id: Option<Option<String>>,
    /// How the site's requests are logged; absent keeps the current settings.
    #[serde(default)]
    pub access_log: Option<AccessLog>,
    /// Rules every request runs before a route is chosen; absent keeps the
    /// current ones.
    #[serde(default)]
    pub rewrites: Option<Vec<RewriteRule>>,
    /// Pages answering the site's errors; absent keeps the current ones.
    #[serde(default)]
    pub error_pages: Option<ErrorPages>,
    /// `null` takes the site out of maintenance for good; absent keeps the
    /// current settings.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<SiteMaintenance>))]
    pub maintenance: Option<Option<SiteMaintenance>>,
    /// `null` leaves `/robots.txt` to the routes; absent keeps the current
    /// answer.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Robots>))]
    pub robots: Option<Option<Robots>>,
    /// `null` leaves `/favicon.ico` to the routes; absent keeps the current
    /// answer.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Favicon>))]
    pub favicon: Option<Option<Favicon>>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub favorite: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct RouteInput {
    /// Keeps an existing route's identity when a whole site is replaced.
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub priority: u32,
    #[serde(rename = "match")]
    pub matcher: RouteMatch,
    pub action: Action,
    /// Restrictions the route's requests pass after the site's.
    #[serde(default)]
    pub security_policy_id: Option<String>,
    /// Field changes, CORS and compression for the route's requests.
    #[serde(default)]
    pub http_policy_id: Option<String>,
    /// What caches the route's responses in place of its site's policy;
    /// `null` follows the site's and absent keeps the current policy.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub cache_policy_id: Option<Option<String>>,
    /// Whether the route's responses stay out of the cache whatever its site
    /// names; absent keeps the current setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<bool>,
    /// How the route's requests are logged; absent keeps the current settings.
    #[serde(default)]
    pub access_log: Option<AccessLog>,
    /// Rules the route's requests run once it is chosen; absent keeps the
    /// current ones.
    #[serde(default)]
    pub rewrites: Option<Vec<RewriteRule>>,
    /// Whether only requests sent from inside the gateway reach the route;
    /// absent keeps the current setting.
    #[serde(default)]
    pub internal: Option<bool>,
    /// Pages answering the route's errors in place of its site's; `null`
    /// answers them with the site's and absent keeps the current ones.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<ErrorPages>))]
    pub error_pages: Option<Option<ErrorPages>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct UpstreamInput {
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<NodeInput>,
    #[serde(default = "round_robin")]
    pub balancing: LoadBalancingPolicy,
    #[serde(default)]
    pub host_header: Option<String>,
    #[serde(default)]
    pub tls: UpstreamTlsPolicy,
    #[serde(default)]
    pub connection: UpstreamConnectionPolicy,
    #[serde(default)]
    pub health_check: Option<ActiveHealthCheck>,
    #[serde(default)]
    pub passive_health: Option<PassiveHealthPolicy>,
    #[serde(default)]
    pub retry: Option<UpstreamRetry>,
    #[serde(default)]
    pub circuit_breaker: Option<CircuitBreaker>,
    #[serde(default)]
    pub max_requests: Option<u32>,
    #[serde(default)]
    pub queue: Option<UpstreamQueue>,
    #[serde(default)]
    pub note: Option<String>,
}

fn round_robin() -> LoadBalancingPolicy {
    LoadBalancingPolicy::RoundRobin
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NodeInput {
    /// Keeps an existing node's identity when a whole upstream is replaced.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    #[serde(default = "one")]
    pub weight: u32,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub backup: bool,
    #[serde(default)]
    pub sni: Option<String>,
    #[serde(default)]
    pub unix_socket: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Sites with the upstreams and TLS profiles they reference, for export.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SiteBundle {
    pub format: String,
    pub sites: Vec<Site>,
    #[serde(default)]
    pub upstreams: Vec<Upstream>,
    #[serde(default)]
    pub tls_profiles: Vec<TlsProfile>,
}

/// Runs `edit` on a copy and returns it only if it introduces no new
/// validation error; existing problems do not block unrelated changes.
pub fn checked<T>(
    model: &ConfigModel,
    edit: impl FnOnce(&mut ConfigModel) -> Result<T>,
) -> Result<(ConfigModel, T)> {
    let before: BTreeSet<(Option<String>, String)> = validate(model)
        .into_iter()
        .map(|diagnostic| (diagnostic.resource_id, diagnostic.message))
        .collect();
    let mut next = model.clone();
    let value = edit(&mut next)?;
    let introduced: Vec<Diagnostic> = validate(&next)
        .into_iter()
        .filter(|diagnostic| {
            !before.contains(&(diagnostic.resource_id.clone(), diagnostic.message.clone()))
        })
        .collect();
    if introduced.is_empty() {
        Ok((next, value))
    } else {
        Err(
            PanelError::validation_failed("the change would make the configuration invalid")
                .with_diagnostics(introduced),
        )
    }
}

fn not_found(kind: &str, id: impl std::fmt::Display) -> PanelError {
    PanelError::not_found(format!("{kind} {id} does not exist"))
}

fn routes_from(inputs: Vec<RouteInput>, existing: &[Route]) -> Vec<Route> {
    inputs
        .into_iter()
        .map(|input| {
            let kept = input
                .id
                .and_then(|id| existing.iter().find(|route| route.id == id));
            Route {
                lua: kept.map(|route| route.lua.clone()).unwrap_or_default(),
                named: kept.and_then(|route| route.named.clone()),
                id: kept.map_or_else(Uuid::now_v7, |route| route.id),
                name: input.name,
                enabled: input.enabled,
                priority: input.priority,
                matcher: input.matcher,
                action: input.action,
                security_policy_id: input.security_policy_id,
                http_policy_id: input.http_policy_id,
                cache_policy_id: input
                    .cache_policy_id
                    .unwrap_or_else(|| kept.and_then(|route| route.cache_policy_id.clone())),
                no_cache: input
                    .no_cache
                    .unwrap_or_else(|| kept.is_some_and(|route| route.no_cache)),
                access_log: input.access_log.unwrap_or_else(|| {
                    kept.map(|route| route.access_log.clone())
                        .unwrap_or_default()
                }),
                rewrites: input.rewrites.unwrap_or_else(|| {
                    kept.map(|route| route.rewrites.clone()).unwrap_or_default()
                }),
                internal: input
                    .internal
                    .unwrap_or_else(|| kept.is_some_and(|route| route.internal)),
                error_pages: input
                    .error_pages
                    .unwrap_or_else(|| kept.and_then(|route| route.error_pages.clone())),
            }
        })
        .collect()
}

fn nodes_from(inputs: Vec<NodeInput>, existing: &[UpstreamNode]) -> Vec<UpstreamNode> {
    inputs
        .into_iter()
        .map(|input| UpstreamNode {
            id: input
                .id
                .filter(|id| existing.iter().any(|node| node.id == *id))
                .unwrap_or_else(Uuid::now_v7),
            host: input.host,
            port: input.port,
            tls: input.tls,
            weight: input.weight,
            enabled: input.enabled,
            backup: input.backup,
            sni: input.sni,
            unix_socket: input.unix_socket,
            note: input.note,
        })
        .collect()
}

impl ConfigModel {
    pub fn site(&self, id: Uuid) -> Result<&Site> {
        self.sites
            .iter()
            .find(|site| site.id == id)
            .ok_or_else(|| not_found("site", id))
    }

    fn site_mut(&mut self, id: Uuid, now: DateTime<Utc>) -> Result<&mut Site> {
        let site = self
            .sites
            .iter_mut()
            .find(|site| site.id == id)
            .ok_or_else(|| not_found("site", id))?;
        site.updated_at = now;
        Ok(site)
    }

    /// Like `site_mut`, for operations the recycle bin forbids.
    fn live_site_mut(&mut self, id: Uuid, now: DateTime<Utc>) -> Result<&mut Site> {
        let site = self.site_mut(id, now)?;
        if site.is_deleted() {
            return Err(PanelError::precondition_failed(format!(
                "site {id} is in the recycle bin; restore it first"
            )));
        }
        Ok(site)
    }

    pub fn create_site(&mut self, input: SiteInput, now: DateTime<Utc>) -> Uuid {
        let id = Uuid::now_v7();
        self.sites.push(Site {
            lua: Default::default(),
            id,
            name: input.name,
            action: input.action,
            enabled: input.enabled,
            domains: input.domains,
            routes: routes_from(input.routes, &[]),
            listener_ids: input.listener_ids,
            https_redirect: input.https_redirect,
            www_redirect: input.www_redirect,
            tls_profile_id: input.tls_profile_id,
            hsts: input.hsts,
            security_policy_id: input.security_policy_id,
            http_policy_id: input.http_policy_id,
            cache_policy_id: input.cache_policy_id.flatten(),
            access_log: input.access_log.unwrap_or_default(),
            rewrites: input.rewrites.unwrap_or_default(),
            error_pages: input.error_pages.unwrap_or_default(),
            maintenance: input.maintenance.flatten(),
            robots: input.robots.flatten(),
            favicon: input.favicon.flatten(),
            group: input.group,
            tags: input.tags,
            note: input.note,
            favorite: input.favorite,
            deleted_at: None,
            created_at: now,
            updated_at: now,
        });
        id
    }

    pub fn replace_site(&mut self, id: Uuid, input: SiteInput, now: DateTime<Utc>) -> Result<()> {
        let site = self.live_site_mut(id, now)?;
        let routes = routes_from(input.routes, &site.routes);
        *site = Site {
            lua: std::mem::take(&mut site.lua),
            id,
            name: input.name,
            action: input.action,
            enabled: input.enabled,
            domains: input.domains,
            routes,
            listener_ids: input.listener_ids,
            https_redirect: input.https_redirect,
            www_redirect: input.www_redirect,
            tls_profile_id: input.tls_profile_id,
            hsts: input.hsts,
            security_policy_id: input.security_policy_id,
            http_policy_id: input.http_policy_id,
            cache_policy_id: input
                .cache_policy_id
                .unwrap_or_else(|| site.cache_policy_id.take()),
            access_log: input.access_log.unwrap_or_else(|| site.access_log.clone()),
            rewrites: input
                .rewrites
                .unwrap_or_else(|| std::mem::take(&mut site.rewrites)),
            error_pages: input
                .error_pages
                .unwrap_or_else(|| std::mem::take(&mut site.error_pages)),
            maintenance: input.maintenance.unwrap_or_else(|| site.maintenance.take()),
            robots: input.robots.unwrap_or_else(|| site.robots.take()),
            favicon: input.favicon.unwrap_or_else(|| site.favicon.take()),
            group: input.group,
            tags: input.tags,
            note: input.note,
            favorite: input.favorite,
            deleted_at: None,
            created_at: site.created_at,
            updated_at: now,
        };
        Ok(())
    }

    pub fn set_site_enabled(&mut self, id: Uuid, enabled: bool, now: DateTime<Utc>) -> Result<()> {
        self.live_site_mut(id, now)?.enabled = enabled;
        Ok(())
    }

    pub fn set_site_favorite(
        &mut self,
        id: Uuid,
        favorite: bool,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.live_site_mut(id, now)?.favorite = favorite;
        Ok(())
    }

    /// Moves the site to the recycle bin; it stops serving and releases its
    /// name and domains until restored.
    pub fn delete_site(&mut self, id: Uuid, now: DateTime<Utc>) -> Result<()> {
        let site = self.site_mut(id, now)?;
        if site.deleted_at.is_none() {
            site.deleted_at = Some(now);
        }
        for listener in &mut self.listeners {
            if listener.default_site_id == Some(id) {
                listener.default_site_id = None;
            }
        }
        Ok(())
    }

    pub fn restore_site(&mut self, id: Uuid, now: DateTime<Utc>) -> Result<()> {
        self.site_mut(id, now)?.deleted_at = None;
        Ok(())
    }

    /// Removes the site for good.
    pub fn purge_site(&mut self, id: Uuid) -> Result<()> {
        self.site(id)?;
        self.sites.retain(|site| site.id != id);
        for listener in &mut self.listeners {
            if listener.default_site_id == Some(id) {
                listener.default_site_id = None;
            }
        }
        Ok(())
    }

    /// Copies settings and routes; domains stay with the original.
    pub fn clone_site(&mut self, id: Uuid, name: String, now: DateTime<Utc>) -> Result<Uuid> {
        let source = self.site(id)?.clone();
        let clone_id = Uuid::now_v7();
        self.sites.push(Site {
            id: clone_id,
            name,
            domains: Vec::new(),
            routes: source
                .routes
                .into_iter()
                .map(|route| Route {
                    id: Uuid::now_v7(),
                    ..route
                })
                .collect(),
            deleted_at: None,
            favorite: false,
            created_at: now,
            updated_at: now,
            ..source
        });
        Ok(clone_id)
    }

    /// Binds domains to a site; any duplicate refuses the whole batch.
    pub fn add_domains(
        &mut self,
        id: Uuid,
        domains: Vec<Domain>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let site = self.live_site_mut(id, now)?;
        site.domains.extend(domains);
        Ok(())
    }

    pub fn replace_domain(
        &mut self,
        id: Uuid,
        host: &NormalizedHost,
        domain: Domain,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let site = self.live_site_mut(id, now)?;
        let primary = domain.primary;
        let slot = site
            .domains
            .iter_mut()
            .find(|candidate| candidate.host == *host)
            .ok_or_else(|| not_found("domain", host))?;
        *slot = domain;
        if primary {
            // Promoting one domain demotes the previous primary.
            let promoted = slot.host.clone();
            for other in site
                .domains
                .iter_mut()
                .filter(|other| other.host != promoted)
            {
                other.primary = false;
            }
        }
        Ok(())
    }

    pub fn remove_domain(
        &mut self,
        id: Uuid,
        host: &NormalizedHost,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let site = self.live_site_mut(id, now)?;
        let before = site.domains.len();
        site.domains.retain(|domain| domain.host != *host);
        if site.domains.len() == before {
            return Err(not_found("domain", host));
        }
        Ok(())
    }

    /// The site owning a route, by route id.
    pub fn route_site(&self, route: Uuid) -> Result<&Site> {
        self.sites
            .iter()
            .find(|site| site.routes.iter().any(|candidate| candidate.id == route))
            .ok_or_else(|| not_found("route", route))
    }

    pub fn create_route(
        &mut self,
        site: Uuid,
        input: RouteInput,
        now: DateTime<Utc>,
    ) -> Result<Uuid> {
        let site = self.live_site_mut(site, now)?;
        let mut routes = routes_from(vec![RouteInput { id: None, ..input }], &[]);
        let id = routes[0].id;
        site.routes.append(&mut routes);
        Ok(id)
    }

    pub fn replace_route(
        &mut self,
        route: Uuid,
        input: RouteInput,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let site_id = self.route_site(route)?.id;
        let site = self.live_site_mut(site_id, now)?;
        let slot = site
            .routes
            .iter_mut()
            .find(|candidate| candidate.id == route)
            .expect("route_site found the route");
        let access_log = input.access_log.unwrap_or_else(|| slot.access_log.clone());
        *slot = Route {
            lua: std::mem::take(&mut slot.lua),
            named: slot.named.take(),
            id: route,
            name: input.name,
            enabled: input.enabled,
            priority: input.priority,
            matcher: input.matcher,
            action: input.action,
            security_policy_id: input.security_policy_id,
            http_policy_id: input.http_policy_id,
            cache_policy_id: input
                .cache_policy_id
                .unwrap_or_else(|| slot.cache_policy_id.take()),
            no_cache: input.no_cache.unwrap_or(slot.no_cache),
            access_log,
            rewrites: input
                .rewrites
                .unwrap_or_else(|| std::mem::take(&mut slot.rewrites)),
            internal: input.internal.unwrap_or(slot.internal),
            error_pages: input.error_pages.unwrap_or_else(|| slot.error_pages.take()),
        };
        Ok(())
    }

    pub fn delete_route(&mut self, route: Uuid, now: DateTime<Utc>) -> Result<()> {
        let site_id = self.route_site(route)?.id;
        self.live_site_mut(site_id, now)?
            .routes
            .retain(|candidate| candidate.id != route);
        Ok(())
    }

    /// Assigns priorities 10, 20, … in the given order, which must name every
    /// route of the site exactly once.
    pub fn reorder_routes(&mut self, site: Uuid, order: &[Uuid], now: DateTime<Utc>) -> Result<()> {
        let site = self.live_site_mut(site, now)?;
        let current: BTreeSet<Uuid> = site.routes.iter().map(|route| route.id).collect();
        let requested: BTreeSet<Uuid> = order.iter().copied().collect();
        if current != requested || requested.len() != order.len() {
            return Err(PanelError::invalid_argument(
                "the order must list every route of the site exactly once",
            ));
        }
        let rank: HashMap<Uuid, u32> = order
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, (index as u32 + 1) * 10))
            .collect();
        for route in &mut site.routes {
            route.priority = rank[&route.id];
        }
        site.routes.sort_by_key(|route| route.priority);
        Ok(())
    }

    pub fn upstream(&self, id: Uuid) -> Result<&Upstream> {
        self.upstreams
            .iter()
            .find(|upstream| upstream.id == id)
            .ok_or_else(|| not_found("upstream", id))
    }

    fn upstream_mut(&mut self, id: Uuid, now: DateTime<Utc>) -> Result<&mut Upstream> {
        let upstream = self
            .upstreams
            .iter_mut()
            .find(|upstream| upstream.id == id)
            .ok_or_else(|| not_found("upstream", id))?;
        upstream.updated_at = now;
        Ok(upstream)
    }

    pub fn create_upstream(&mut self, input: UpstreamInput, now: DateTime<Utc>) -> Uuid {
        let id = Uuid::now_v7();
        self.upstreams.push(Upstream {
            balancer: Default::default(),
            id,
            name: input.name,
            nodes: nodes_from(input.nodes, &[]),
            balancing: input.balancing,
            host_header: input.host_header,
            tls: input.tls,
            connection: input.connection,
            health_check: input.health_check,
            passive_health: input.passive_health,
            retry: input.retry,
            circuit_breaker: input.circuit_breaker,
            max_requests: input.max_requests,
            queue: input.queue,
            note: input.note,
            created_at: now,
            updated_at: now,
        });
        id
    }

    pub fn replace_upstream(
        &mut self,
        id: Uuid,
        input: UpstreamInput,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let upstream = self.upstream_mut(id, now)?;
        let nodes = nodes_from(input.nodes, &upstream.nodes);
        *upstream = Upstream {
            balancer: upstream.balancer.take(),
            id,
            name: input.name,
            nodes,
            balancing: input.balancing,
            host_header: input.host_header,
            tls: input.tls,
            connection: input.connection,
            health_check: input.health_check,
            passive_health: input.passive_health,
            retry: input.retry,
            circuit_breaker: input.circuit_breaker,
            max_requests: input.max_requests,
            queue: input.queue,
            note: input.note,
            created_at: upstream.created_at,
            updated_at: now,
        };
        Ok(())
    }

    /// Refused while any site, including one in the recycle bin, uses it.
    pub fn delete_upstream(&mut self, id: Uuid) -> Result<()> {
        self.upstream(id)?;
        let users: Vec<&str> = self
            .sites
            .iter()
            .filter(|site| {
                std::iter::once(&site.action)
                    .chain(site.routes.iter().map(|route| &route.action))
                    .any(|action| matches!(action, Action::Proxy { upstream_id } if *upstream_id == id))
            })
            .map(|site| site.name.as_str())
            .collect();
        if !users.is_empty() {
            return Err(PanelError::conflict(format!(
                "upstream {id} is used by sites: {}",
                users.join(", ")
            )));
        }
        self.upstreams.retain(|upstream| upstream.id != id);
        Ok(())
    }

    pub fn add_node(
        &mut self,
        upstream: Uuid,
        input: NodeInput,
        now: DateTime<Utc>,
    ) -> Result<Uuid> {
        let upstream = self.upstream_mut(upstream, now)?;
        let mut nodes = nodes_from(vec![NodeInput { id: None, ..input }], &[]);
        let id = nodes[0].id;
        upstream.nodes.append(&mut nodes);
        Ok(id)
    }

    pub fn replace_node(
        &mut self,
        upstream: Uuid,
        node: Uuid,
        input: NodeInput,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let upstream = self.upstream_mut(upstream, now)?;
        let slot = upstream
            .nodes
            .iter_mut()
            .find(|candidate| candidate.id == node)
            .ok_or_else(|| not_found("node", node))?;
        *slot = nodes_from(
            vec![NodeInput {
                id: Some(node),
                ..input
            }],
            std::slice::from_ref(slot),
        )
        .remove(0);
        Ok(())
    }

    pub fn delete_node(&mut self, upstream: Uuid, node: Uuid, now: DateTime<Utc>) -> Result<()> {
        let upstream = self.upstream_mut(upstream, now)?;
        let before = upstream.nodes.len();
        upstream.nodes.retain(|candidate| candidate.id != node);
        if upstream.nodes.len() == before {
            return Err(not_found("node", node));
        }
        Ok(())
    }

    /// Creates or replaces a listener; returns whether it was created.
    pub fn put_listener(&mut self, listener: Listener) -> bool {
        match self
            .listeners
            .iter_mut()
            .find(|item| item.id == listener.id)
        {
            Some(slot) => {
                *slot = listener;
                false
            }
            None => {
                self.listeners.push(listener);
                true
            }
        }
    }

    pub fn delete_listener(&mut self, id: &str) -> Result<()> {
        if !self.listeners.iter().any(|listener| listener.id == id) {
            return Err(not_found("listener", id));
        }
        let users: Vec<&str> = self
            .sites
            .iter()
            .filter(|site| site.listener_ids.contains(id))
            .map(|site| site.name.as_str())
            .collect();
        if !users.is_empty() {
            return Err(PanelError::conflict(format!(
                "listener {id} is used by sites: {}",
                users.join(", ")
            )));
        }
        self.listeners.retain(|listener| listener.id != id);
        Ok(())
    }

    /// Creates or replaces a TLS profile; returns whether it was created.
    pub fn put_tls_profile(&mut self, profile: TlsProfile) -> bool {
        match self
            .tls_profiles
            .iter_mut()
            .find(|item| item.id == profile.id)
        {
            Some(slot) => {
                *slot = profile;
                false
            }
            None => {
                self.tls_profiles.push(profile);
                true
            }
        }
    }

    /// Creates or replaces a security policy; returns whether it was created.
    pub fn put_security_policy(&mut self, policy: SecurityPolicy) -> bool {
        match self
            .security_policies
            .iter_mut()
            .find(|item| item.id == policy.id)
        {
            Some(slot) => {
                *slot = policy;
                false
            }
            None => {
                self.security_policies.push(policy);
                true
            }
        }
    }

    pub fn delete_security_policy(&mut self, id: &str) -> Result<()> {
        if !self.security_policies.iter().any(|policy| policy.id == id) {
            return Err(not_found("security policy", id));
        }
        let used = self.sites.iter().any(|site| {
            site.security_policy_id.as_deref() == Some(id)
                || site
                    .routes
                    .iter()
                    .any(|route| route.security_policy_id.as_deref() == Some(id))
        });
        if used {
            return Err(PanelError::conflict(format!(
                "security policy {id} is still in use"
            )));
        }
        self.security_policies.retain(|policy| policy.id != id);
        Ok(())
    }

    /// Creates or replaces an HTTP policy; returns whether it was created.
    pub fn put_http_policy(&mut self, policy: HttpPolicy) -> bool {
        match self
            .http_policies
            .iter_mut()
            .find(|item| item.id == policy.id)
        {
            Some(slot) => {
                *slot = policy;
                false
            }
            None => {
                self.http_policies.push(policy);
                true
            }
        }
    }

    pub fn delete_http_policy(&mut self, id: &str) -> Result<()> {
        if !self.http_policies.iter().any(|policy| policy.id == id) {
            return Err(not_found("HTTP policy", id));
        }
        let used = self.sites.iter().any(|site| {
            site.http_policy_id.as_deref() == Some(id)
                || site
                    .routes
                    .iter()
                    .any(|route| route.http_policy_id.as_deref() == Some(id))
        });
        if used {
            return Err(PanelError::conflict(format!(
                "HTTP policy {id} is still in use"
            )));
        }
        self.http_policies.retain(|policy| policy.id != id);
        Ok(())
    }

    pub fn put_cache_policy(&mut self, policy: CachePolicy) -> bool {
        match self
            .cache_policies
            .iter_mut()
            .find(|item| item.id == policy.id)
        {
            Some(slot) => {
                *slot = policy;
                false
            }
            None => {
                self.cache_policies.push(policy);
                true
            }
        }
    }

    pub fn delete_cache_policy(&mut self, id: &str) -> Result<()> {
        if !self.cache_policies.iter().any(|policy| policy.id == id) {
            return Err(not_found("cache policy", id));
        }
        let used = self.sites.iter().any(|site| {
            site.cache_policy_id.as_deref() == Some(id)
                || site
                    .routes
                    .iter()
                    .any(|route| route.cache_policy_id.as_deref() == Some(id))
        });
        if used {
            return Err(PanelError::conflict(format!(
                "cache policy {id} is still in use"
            )));
        }
        self.cache_policies.retain(|policy| policy.id != id);
        Ok(())
    }

    pub fn delete_tls_profile(&mut self, id: &str) -> Result<()> {
        if !self.tls_profiles.iter().any(|profile| profile.id == id) {
            return Err(not_found("TLS profile", id));
        }
        let used = self
            .listeners
            .iter()
            .any(|listener| listener.tls_profile_id.as_deref() == Some(id))
            || self.sites.iter().any(|site| {
                site.tls_profile_id.as_deref() == Some(id)
                    || site
                        .domains
                        .iter()
                        .any(|domain| domain.tls_profile_id.as_deref() == Some(id))
            });
        if used {
            return Err(PanelError::conflict(format!(
                "TLS profile {id} is still in use"
            )));
        }
        self.tls_profiles.retain(|profile| profile.id != id);
        Ok(())
    }

    /// Sites (all live ones when `ids` is empty) with what they reference.
    pub fn export_sites(&self, ids: &[Uuid]) -> Result<SiteBundle> {
        let sites: Vec<Site> = if ids.is_empty() {
            self.sites
                .iter()
                .filter(|site| !site.is_deleted())
                .cloned()
                .collect()
        } else {
            ids.iter()
                .map(|id| self.site(*id).cloned())
                .collect::<Result<_>>()?
        };
        let upstreams: BTreeSet<Uuid> = sites
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
        let profiles: BTreeSet<&str> = sites
            .iter()
            .flat_map(|site| {
                site.tls_profile_id.as_deref().into_iter().chain(
                    site.domains
                        .iter()
                        .filter_map(|domain| domain.tls_profile_id.as_deref()),
                )
            })
            .collect();
        Ok(SiteBundle {
            format: MODEL_VERSION.into(),
            upstreams: self
                .upstreams
                .iter()
                .filter(|upstream| upstreams.contains(&upstream.id))
                .cloned()
                .collect(),
            tls_profiles: self
                .tls_profiles
                .iter()
                .filter(|profile| profiles.contains(profile.id.as_str()))
                .cloned()
                .collect(),
            sites,
        })
    }

    /// Imports a bundle as new sites and upstreams with fresh identities.
    /// TLS profiles are matched by id and must agree with existing ones.
    pub fn import_sites(&mut self, bundle: SiteBundle, now: DateTime<Utc>) -> Result<Vec<Uuid>> {
        if bundle.format != MODEL_VERSION {
            return Err(PanelError::unsupported_capability(format!(
                "bundle format {} is not {MODEL_VERSION}",
                bundle.format
            )));
        }
        for profile in bundle.tls_profiles {
            match self
                .tls_profiles
                .iter()
                .find(|existing| existing.id == profile.id)
            {
                Some(existing) if *existing != profile => {
                    return Err(PanelError::conflict(format!(
                        "TLS profile {} differs from the existing one",
                        profile.id
                    )))
                }
                Some(_) => {}
                None => self.tls_profiles.push(profile),
            }
        }
        let mut remapped = HashMap::new();
        for upstream in bundle.upstreams {
            let id = Uuid::now_v7();
            remapped.insert(upstream.id, id);
            self.upstreams.push(Upstream {
                id,
                nodes: upstream
                    .nodes
                    .into_iter()
                    .map(|node| UpstreamNode {
                        id: Uuid::now_v7(),
                        ..node
                    })
                    .collect(),
                created_at: now,
                updated_at: now,
                ..upstream
            });
        }
        let remap = |action: Action| match action {
            Action::Proxy { upstream_id } => Action::Proxy {
                upstream_id: remapped.get(&upstream_id).copied().unwrap_or(upstream_id),
            },
            other => other,
        };
        let mut created = Vec::with_capacity(bundle.sites.len());
        for site in bundle.sites {
            let id = Uuid::now_v7();
            created.push(id);
            self.sites.push(Site {
                id,
                action: remap(site.action),
                routes: site
                    .routes
                    .into_iter()
                    .map(|route| Route {
                        id: Uuid::now_v7(),
                        action: remap(route.action),
                        ..route
                    })
                    .collect(),
                deleted_at: None,
                created_at: now,
                updated_at: now,
                ..site
            });
        }
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MatchKind;
    use panel_ir::ListenerProtocols;

    fn input(name: &str, hosts: &[&str], action: Action) -> SiteInput {
        SiteInput {
            cache_policy_id: None,
            error_pages: None,
            maintenance: None,
            robots: None,
            favicon: None,
            name: name.into(),
            action,
            enabled: true,
            domains: hosts
                .iter()
                .map(|host| Domain {
                    host: NormalizedHost::new(host).unwrap(),
                    enabled: true,
                    primary: false,
                    redirect: false,
                    tls_profile_id: None,
                })
                .collect(),
            routes: Vec::new(),
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            tls_profile_id: None,
            hsts: None,
            group: None,
            tags: BTreeSet::new(),
            note: None,
            favorite: false,
            security_policy_id: Default::default(),
            http_policy_id: None,
            access_log: None,
            rewrites: None,
        }
    }

    fn replaced_route(route: &Route) -> RouteInput {
        RouteInput {
            cache_policy_id: None,
            no_cache: None,
            error_pages: None,
            id: Some(route.id),
            name: route.name.clone(),
            enabled: route.enabled,
            priority: route.priority,
            matcher: route.matcher.clone(),
            action: route.action.clone(),
            security_policy_id: None,
            http_policy_id: None,
            access_log: None,
            rewrites: None,
            internal: None,
        }
    }

    fn maintenance() -> Action {
        Action::Respond {
            status: 503,
            body: Some("soon".into()),
            content_type: None,
            retry_after_seconds: None,
        }
    }

    fn upstream_input() -> UpstreamInput {
        UpstreamInput {
            name: "app".into(),
            nodes: vec![NodeInput {
                id: None,
                host: "10.0.0.5".into(),
                port: 8080,
                tls: false,
                weight: 1,
                enabled: true,
                backup: false,
                sni: None,
                unix_socket: None,
                note: Some("rack 1".into()),
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
        }
    }

    #[test]
    fn form_edits_keep_logging_and_lua_settings() {
        let now = Utc::now();
        let mut model = ConfigModel::default();
        let mut created = input("shop", &["shop.example.com"], maintenance());
        created.access_log = Some(AccessLog {
            enabled: Some(false),
            ..AccessLog::default()
        });
        created.routes.push(RouteInput {
            cache_policy_id: None,
            no_cache: None,
            error_pages: None,
            id: None,
            name: Some("api".into()),
            enabled: true,
            priority: 10,
            matcher: crate::model::RouteMatch {
                kind: MatchKind::Prefix,
                path: "/api".into(),
                host: None,
                conditions: Vec::new(),
            },
            action: maintenance(),
            security_policy_id: None,
            http_policy_id: None,
            access_log: Some(AccessLog {
                format: Some(panel_ir::AccessLogFormat::Combined),
                ..AccessLog::default()
            }),
            rewrites: None,
            internal: None,
        });
        let id = model.create_site(created, now);
        {
            let site = model.sites.iter_mut().find(|site| site.id == id).unwrap();
            site.lua.access = Some(crate::LuaCode::inline("ngx.exit(403)"));
            site.routes[0].lua.debug = Some(true);
        }
        let site = model.site(id).unwrap().clone();
        assert_eq!(site.access_log.enabled, Some(false));

        let mut replaced = input("shop", &["shop.example.com"], maintenance());
        let route = &site.routes[0];
        replaced.routes.push(RouteInput {
            cache_policy_id: None,
            no_cache: None,
            error_pages: None,
            id: Some(route.id),
            name: route.name.clone(),
            enabled: true,
            priority: route.priority,
            matcher: route.matcher.clone(),
            action: route.action.clone(),
            security_policy_id: None,
            http_policy_id: None,
            access_log: None,
            rewrites: None,
            internal: None,
        });
        model.replace_site(id, replaced, now).unwrap();
        let kept = model.site(id).unwrap();
        assert_eq!(kept.access_log, site.access_log);
        assert_eq!(kept.routes[0].access_log, route.access_log);
        assert_eq!(kept.lua, site.lua);
        assert_eq!(kept.routes[0].lua, route.lua);
        let route_id = route.id;
        let mut route_input = replaced_route(&kept.routes[0]);
        route_input.name = Some("api-2".into());
        model.replace_route(route_id, route_input, now).unwrap();
        assert_eq!(model.site(id).unwrap().routes[0].lua, route.lua);

        let mut cleared = input("shop", &["shop.example.com"], maintenance());
        cleared.access_log = Some(AccessLog::default());
        model.replace_site(id, cleared, now).unwrap();
        assert!(model.site(id).unwrap().access_log.is_unset());
    }

    #[test]
    fn edits_that_introduce_errors_leave_the_model_unchanged() {
        let now = Utc::now();
        let model = ConfigModel::default();
        let (model, first) = checked(&model, |model| {
            Ok(model.create_site(input("shop", &["shop.example.com"], maintenance()), now))
        })
        .unwrap();
        let error = checked(&model, |model| {
            Ok(model.create_site(input("other", &["SHOP.example.com"], maintenance()), now))
        })
        .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::VALIDATION_FAILED
        );
        assert!(error.diagnostics[0].message.contains("already bound"));
        assert_eq!(model.sites.len(), 1);
        assert_eq!(model.site(first).unwrap().name, "shop");
    }

    #[test]
    fn recycle_bin_round_trip_and_clone() {
        let now = Utc::now();
        let mut model = ConfigModel::default();
        let id = model.create_site(input("shop", &["shop.example.com"], maintenance()), now);
        model.put_listener(Listener {
            id: "http".into(),
            address: "0.0.0.0:80".into(),
            tls_profile_id: None,
            protocols: ListenerProtocols::default(),
            reuse_port: false,
            ipv6_only: None,
            default_site_id: Some(id),
            real_ip_header: Default::default(),
            trusted_proxies: Default::default(),
            request_head_timeout_seconds: Default::default(),
        });
        model.delete_site(id, now).unwrap();
        assert!(model.site(id).unwrap().is_deleted());
        assert!(model.listeners[0].default_site_id.is_none());
        assert!(model.set_site_enabled(id, false, now).is_err());
        model.restore_site(id, now).unwrap();
        model.set_site_enabled(id, false, now).unwrap();
        let copy = model.clone_site(id, "shop copy".into(), now).unwrap();
        assert!(model.site(copy).unwrap().domains.is_empty());
        assert!(!model.site(copy).unwrap().enabled);
        model.purge_site(id).unwrap();
        assert!(model.site(id).is_err());
    }

    #[test]
    fn routes_reorder_and_upstreams_guard_their_users() {
        let now = Utc::now();
        let mut model = ConfigModel::default();
        let upstream = model.create_upstream(upstream_input(), now);
        let site = model.create_site(
            input(
                "shop",
                &["shop.example.com"],
                Action::Proxy {
                    upstream_id: upstream,
                },
            ),
            now,
        );
        let route = |path: &str| RouteInput {
            cache_policy_id: None,
            no_cache: None,
            error_pages: None,
            id: None,
            name: None,
            enabled: true,
            priority: 100,
            matcher: RouteMatch {
                kind: MatchKind::Prefix,
                path: path.into(),
                host: None,
                conditions: Vec::new(),
            },
            action: maintenance(),
            security_policy_id: Default::default(),
            http_policy_id: None,
            access_log: None,
            rewrites: None,
            internal: None,
        };
        let first = model.create_route(site, route("/a"), now).unwrap();
        let second = model.create_route(site, route("/b"), now).unwrap();
        model.reorder_routes(site, &[second, first], now).unwrap();
        let routes = &model.site(site).unwrap().routes;
        assert_eq!((routes[0].id, routes[0].priority), (second, 10));
        assert_eq!((routes[1].id, routes[1].priority), (first, 20));
        assert!(model.reorder_routes(site, &[first], now).is_err());
        assert_eq!(
            model.delete_upstream(upstream).unwrap_err().code.as_str(),
            panel_errors::ErrorCode::CONFLICT
        );
        let node = model.upstream(upstream).unwrap().nodes[0].id;
        let mut replacement = upstream_input();
        replacement.nodes[0].id = Some(node);
        replacement.nodes[0].weight = 5;
        model.replace_upstream(upstream, replacement, now).unwrap();
        assert_eq!(model.upstream(upstream).unwrap().nodes[0].id, node);
        assert_eq!(model.upstream(upstream).unwrap().nodes[0].weight, 5);

        let strip = vec![panel_ir::RewriteRule::StripPrefix {
            prefix: "/a".into(),
        }];
        let mut rewritten = route("/a");
        rewritten.rewrites = Some(strip.clone());
        rewritten.internal = Some(true);
        model.replace_route(first, rewritten, now).unwrap();
        model.replace_route(first, route("/a"), now).unwrap();
        let find = |model: &ConfigModel| {
            model
                .site(site)
                .unwrap()
                .routes
                .iter()
                .find(|route| route.id == first)
                .cloned()
                .unwrap()
        };
        let kept = find(&model);
        assert_eq!((&kept.rewrites, kept.internal), (&strip, true));
        let mut cleared = route("/a");
        cleared.rewrites = Some(Vec::new());
        cleared.internal = Some(false);
        model.replace_route(first, cleared, now).unwrap();
        let cleared = find(&model);
        assert!(cleared.rewrites.is_empty() && !cleared.internal);
    }

    #[test]
    fn export_and_import_copy_sites_with_fresh_identities() {
        let now = Utc::now();
        let mut source = ConfigModel::default();
        let upstream = source.create_upstream(upstream_input(), now);
        source.create_site(
            input(
                "shop",
                &["shop.example.com"],
                Action::Proxy {
                    upstream_id: upstream,
                },
            ),
            now,
        );
        let bundle = source.export_sites(&[]).unwrap();
        assert_eq!(bundle.upstreams.len(), 1);
        let mut target = ConfigModel::default();
        let created = target.import_sites(bundle.clone(), now).unwrap();
        let site = target.site(created[0]).unwrap();
        let Action::Proxy { upstream_id } = site.action else {
            panic!("proxy action expected")
        };
        assert_ne!(upstream_id, upstream);
        assert_eq!(target.upstream(upstream_id).unwrap().name, "app");
        assert!(crate::validate::validate(&target).is_empty());
        let mut wrong = bundle;
        wrong.format = "other/v9".into();
        assert!(target.import_sites(wrong, now).is_err());
    }

    #[test]
    fn promoting_a_primary_domain_demotes_the_previous_one() {
        let now = Utc::now();
        let mut model = ConfigModel::default();
        let site = model.create_site(
            input("shop", &["a.example", "b.example"], maintenance()),
            now,
        );
        let host = |value| NormalizedHost::new(value).unwrap();
        let mut domain = model.site(site).unwrap().domains[0].clone();
        domain.primary = true;
        model
            .replace_domain(site, &host("a.example"), domain, now)
            .unwrap();
        let mut domain = model.site(site).unwrap().domains[1].clone();
        domain.primary = true;
        model
            .replace_domain(site, &host("b.example"), domain, now)
            .unwrap();
        let primaries: Vec<_> = model
            .site(site)
            .unwrap()
            .domains
            .iter()
            .filter(|domain| domain.primary)
            .map(|domain| domain.host.as_str().to_owned())
            .collect();
        assert_eq!(primaries, ["b.example"]);
        model.remove_domain(site, &host("a.example"), now).unwrap();
        assert!(model.remove_domain(site, &host("a.example"), now).is_err());
    }

    #[test]
    fn site_settings_are_kept_unless_written_and_cleared_with_null() {
        use crate::model::{Favicon, Robots};

        let mut model = ConfigModel::default();
        let input: SiteInput = serde_json::from_value(serde_json::json!({
            "name": "shop",
            "action": {"type": "respond", "status": 200},
            "error_pages": {"pages": [{"statuses": [404], "response": {"kind": "body", "body": "gone"}}]},
            "maintenance": {"allow": ["10.0.0.0/8"]},
            "robots": {"kind": "disallow_all"},
            "favicon": {"kind": "no_content"},
            "cache_policy_id": "pages",
            "routes": [{
                "priority": 1,
                "match": {"kind": "prefix", "path": "/api"},
                "action": {"type": "respond", "status": 200},
                "error_pages": {"pages": []},
                "no_cache": true
            }]
        }))
        .unwrap();
        let id = model.create_site(input, Utc::now());
        let site = model.site(id).unwrap();
        let route = site.routes[0].id;
        assert_eq!(site.error_pages.pages.len(), 1);
        let maintenance = site.maintenance.as_ref().unwrap();
        assert!(maintenance.enabled && maintenance.status == 503);
        assert_eq!(site.robots, Some(Robots::DisallowAll));
        assert_eq!(site.favicon, Some(Favicon::NoContent));
        assert_eq!(site.routes[0].error_pages, Some(ErrorPages::default()));
        assert_eq!(site.cache_policy_id.as_deref(), Some("pages"));
        assert!(site.routes[0].no_cache);

        let kept: SiteInput = serde_json::from_value(serde_json::json!({
            "name": "shop",
            "action": {"type": "respond", "status": 200},
            "routes": [{
                "id": route,
                "priority": 1,
                "match": {"kind": "prefix", "path": "/api"},
                "action": {"type": "respond", "status": 200}
            }]
        }))
        .unwrap();
        assert!(kept.maintenance.is_none() && kept.routes[0].error_pages.is_none());
        model.replace_site(id, kept, Utc::now()).unwrap();
        let site = model.site(id).unwrap();
        assert_eq!(site.error_pages.pages.len(), 1);
        assert!(site.maintenance.is_some() && site.robots.is_some() && site.favicon.is_some());
        assert_eq!(site.routes[0].error_pages, Some(ErrorPages::default()));
        assert_eq!(site.cache_policy_id.as_deref(), Some("pages"));
        assert!(site.routes[0].no_cache);

        let cleared: SiteInput = serde_json::from_value(serde_json::json!({
            "name": "shop",
            "action": {"type": "respond", "status": 200},
            "error_pages": {},
            "maintenance": null,
            "robots": null,
            "favicon": null,
            "cache_policy_id": null,
            "routes": [{
                "id": route,
                "priority": 1,
                "match": {"kind": "prefix", "path": "/api"},
                "action": {"type": "respond", "status": 200},
                "error_pages": null,
                "no_cache": false
            }]
        }))
        .unwrap();
        assert_eq!(cleared.maintenance, Some(None));
        let written = serde_json::to_value(&cleared).unwrap();
        assert!(written["maintenance"].is_null() && written.get("maintenance").is_some());
        assert!(serde_json::to_value(kept_input())
            .unwrap()
            .get("robots")
            .is_none());
        model.replace_site(id, cleared, Utc::now()).unwrap();
        let site = model.site(id).unwrap();
        assert!(site.error_pages.is_empty());
        assert!(site.maintenance.is_none() && site.robots.is_none() && site.favicon.is_none());
        assert_eq!(site.routes[0].error_pages, None);
        assert!(site.cache_policy_id.is_none() && !site.routes[0].no_cache);
    }

    fn kept_input() -> SiteInput {
        serde_json::from_value(serde_json::json!({
            "name": "shop",
            "action": {"type": "respond", "status": 200}
        }))
        .unwrap()
    }
}
