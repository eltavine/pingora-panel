//! The sites in front of containers (ADR 0033): which sites point at which
//! containers, read against the draft.

use crate::{
    access::site_scope,
    configuration::port,
    containers::{engine, EndpointRouteName},
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Extension, Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    declared_site, endpoints, same_host, ContainerFilter, ContainerSummary, EndpointRoute,
    RequestScope,
};
use panel_config_api::ModelQuery;
use panel_config_model::{Action, SiteBundle};
use panel_errors::PanelError;
use panel_identity::{Access as HeldAccess, Permission};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use utoipa::ToSchema;
use uuid::Uuid;

/// A site that points at a container.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteLinkView {
    pub container_id: String,
    /// Its first name, without the leading slash.
    pub container: String,
    pub site_id: Uuid,
    pub site: String,
    pub upstream_id: Uuid,
    pub upstream: String,
    /// The node's host and port, such as `127.0.0.1:8081`.
    pub node: String,
    pub route: EndpointRouteName,
}

/// A container whose labels declare hosts no site serves.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct UnservedSiteView {
    pub container_id: String,
    pub container: String,
    /// The declared hosts no readable site has.
    pub domains: Vec<String>,
    /// The container's port the site would proxy to.
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteLinksView {
    /// When the agent read the containers, RFC 3339.
    pub observed_at: Option<String>,
    /// By container, then site.
    pub links: Vec<SiteLinkView>,
    pub unserved: Vec<UnservedSiteView>,
}

fn name(container: &ContainerSummary) -> String {
    container
        .names
        .first()
        .cloned()
        .unwrap_or_else(|| container.id.clone())
}

/// The sites of `bundle` that point at `containers`, and the declarations
/// none of its sites serves.
pub(crate) fn links(
    containers: &[ContainerSummary],
    bundle: &SiteBundle,
) -> (Vec<SiteLinkView>, Vec<UnservedSiteView>) {
    let upstreams: HashMap<Uuid, _> = bundle
        .upstreams
        .iter()
        .map(|upstream| (upstream.id, upstream))
        .collect();
    let served: BTreeSet<&str> = bundle
        .sites
        .iter()
        .flat_map(|site| site.domains.iter().map(|domain| domain.host.as_str()))
        .collect();
    let (mut links, mut unserved) = (Vec::new(), Vec::new());
    for container in containers {
        let reachable = endpoints(container);
        for site in &bundle.sites {
            let proxied: BTreeSet<Uuid> = std::iter::once(&site.action)
                .chain(site.routes.iter().map(|route| &route.action))
                .filter_map(|action| match action {
                    Action::Proxy { upstream_id } => Some(*upstream_id),
                    _ => None,
                })
                .collect();
            for upstream in proxied.iter().filter_map(|id| upstreams.get(id)) {
                for node in &upstream.nodes {
                    let Some(endpoint) = reachable.iter().find(|endpoint| {
                        endpoint.port == node.port && same_host(&node.host, endpoint.address)
                    }) else {
                        continue;
                    };
                    links.push(SiteLinkView {
                        container_id: container.id.clone(),
                        container: name(container),
                        site_id: site.id,
                        site: site.name.clone(),
                        upstream_id: upstream.id,
                        upstream: upstream.name.clone(),
                        node: if node.host.contains(':') {
                            format!("[{}]:{}", node.host, node.port)
                        } else {
                            format!("{}:{}", node.host, node.port)
                        },
                        route: match endpoint.route {
                            EndpointRoute::Network(_) => EndpointRouteName::Network,
                            _ => EndpointRouteName::Published,
                        },
                    });
                }
            }
        }
        if let Some(declared) = declared_site(&container.labels) {
            let domains: Vec<String> = declared
                .domains
                .iter()
                .map(|host| host.as_str())
                .filter(|host| !served.contains(host))
                .map(str::to_owned)
                .collect();
            if !domains.is_empty() {
                unserved.push(UnservedSiteView {
                    container_id: container.id.clone(),
                    container: name(container),
                    domains,
                    port: declared.port,
                });
            }
        }
    }
    links
        .sort_by(|left, right| (&left.container, &left.site).cmp(&(&right.container, &right.site)));
    links.dedup_by(|left, right| {
        (
            &left.container_id,
            left.site_id,
            left.upstream_id,
            &left.node,
        ) == (
            &right.container_id,
            right.site_id,
            right.upstream_id,
            &right.node,
        )
    });
    unserved.sort_by(|left, right| left.container.cmp(&right.container));
    (links, unserved)
}

/// The scope to read the configuration in for a caller with `held`: links
/// name sites, so only a caller who reads sites sees them, and only those
/// it may read. Without an identity every site is readable.
pub(crate) fn configuration_scope(
    held: Option<&HeldAccess>,
    scope: RequestScope,
) -> Result<RequestScope, ApiError> {
    let Some(held) = held else {
        return Ok(scope);
    };
    if !held.holds(Permission::ConfigRead) {
        return Err(ApiError::new(PanelError::permission_denied(
            "site links name sites, which need the config.read permission",
        )));
    }
    Ok(if held.unrestricted.contains(Permission::ConfigRead) {
        scope
    } else {
        scope.with_site_scope(Some(site_scope(held)))
    })
}

/// The sites of the draft that point at an engine's containers, by their
/// upstreams' nodes, and the containers whose labels declare hosts no site
/// serves. Only sites the caller may read are named.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/site-links",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = SiteLinksView)), tag = "containers")]
pub(crate) async fn site_links<U>(
    State(state): State<ApiState<U>>,
    Path(id): Path<String>,
    held: Option<Extension<HeldAccess>>,
    headers: HeaderMap,
) -> Result<Json<SiteLinksView>, ApiError> {
    let scope = request_scope(&headers)?;
    let configuration =
        configuration_scope(held.as_ref().map(|Extension(held)| held), scope.clone())?;
    let list = state
        .containers
        .containers(scope, engine(id)?, ContainerFilter::default())
        .await?;
    let output = port(&state)?
        .read(
            configuration,
            ModelQuery::ExportSites { ids: Vec::new() }.into(),
        )
        .await?;
    let bundle: SiteBundle = serde_json::from_slice(&output.content).map_err(|error| {
        ApiError::new(PanelError::internal(format!(
            "the configuration answered sites it cannot read: {error}"
        )))
    })?;
    let (links, unserved) = links(&list.containers, &bundle);
    Ok(Json(SiteLinksView {
        observed_at: list
            .observed_at
            .map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)),
        links,
        unserved,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use panel_application::{ContainerAddress, ContainerState, PortMapping, RequestId};
    use panel_identity::{GrantScope, PermissionSet};

    /// shop-web-1, published on 8081 and at 172.18.0.2, declaring two hosts.
    pub(crate) fn shop_web() -> ContainerSummary {
        ContainerSummary {
            id: "b2".into(),
            names: vec!["shop-web-1".into()],
            image: "nginx:1.27".into(),
            image_id: String::new(),
            created: None,
            state: ContainerState::Running,
            status: String::new(),
            ports: vec![PortMapping {
                private_port: 80,
                public_port: Some(8081),
                host_ip: "0.0.0.0".into(),
                protocol: "tcp".into(),
            }],
            labels: [(
                "pingora-panel.site.domains".to_owned(),
                "shop.example,api.shop.example".to_owned(),
            )]
            .into(),
            compose_project: None,
            addresses: vec![ContainerAddress {
                network: "shop_default".into(),
                ipv4: Some([172, 18, 0, 2].into()),
                ipv6: None,
            }],
        }
    }

    /// shop proxies to web on localhost:8081 and its /api route to api,
    /// whose nodes are the container's network address and another host;
    /// blog proxies elsewhere.
    pub(crate) fn bundle() -> SiteBundle {
        let at = "2027-01-15T08:00:00Z";
        let id = |n: u8| format!("0190b5b6-3f43-7a52-8a56-2f8b7a7d5a{n:02}");
        serde_json::from_value(serde_json::json!({
            "format": panel_config_model::MODEL_VERSION,
            "sites": [
                {"id": id(1), "name": "shop",
                 "action": {"type": "proxy", "upstream_id": id(11)},
                 "domains": [{"host": "shop.example"}],
                 "routes": [{"id": id(21), "priority": 10,
                             "match": {"kind": "prefix", "path": "/api"},
                             "action": {"type": "proxy", "upstream_id": id(12)}}],
                 "created_at": at, "updated_at": at},
                {"id": id(2), "name": "blog",
                 "action": {"type": "proxy", "upstream_id": id(13)},
                 "domains": [{"host": "blog.example"}],
                 "created_at": at, "updated_at": at}
            ],
            "upstreams": [
                {"id": id(11), "name": "web",
                 "nodes": [{"id": id(31), "host": "localhost", "port": 8081}],
                 "created_at": at, "updated_at": at},
                {"id": id(12), "name": "api",
                 "nodes": [{"id": id(32), "host": "172.18.0.2", "port": 80},
                           {"id": id(33), "host": "10.0.0.9", "port": 80}],
                 "created_at": at, "updated_at": at},
                {"id": id(13), "name": "blog",
                 "nodes": [{"id": id(34), "host": "127.0.0.1", "port": 9999}],
                 "created_at": at, "updated_at": at}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn sites_point_at_containers_by_their_upstreams_nodes() {
        let (links, unserved) = links(&[shop_web()], &bundle());
        let found: Vec<_> = links
            .iter()
            .map(|link| {
                (
                    link.container.as_str(),
                    link.site.as_str(),
                    link.upstream.as_str(),
                    link.node.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("shop-web-1", "shop", "web", "localhost:8081"),
                ("shop-web-1", "shop", "api", "172.18.0.2:80"),
            ]
        );
        assert!(matches!(links[0].route, EndpointRouteName::Published));
        assert!(matches!(links[1].route, EndpointRouteName::Network));
        assert_eq!(unserved.len(), 1);
        assert_eq!(unserved[0].domains, ["api.shop.example"]);

        let mut stopped = shop_web();
        stopped.state = ContainerState::Exited;
        assert!(
            links_of(&stopped).is_empty(),
            "a stopped container is reached nowhere"
        );
    }

    fn links_of(container: &ContainerSummary) -> Vec<SiteLinkView> {
        links(std::slice::from_ref(container), &bundle()).0
    }

    #[test]
    fn links_name_only_the_sites_their_caller_reads() {
        let scope = || RequestScope::new(RequestId::new("request-1").unwrap());
        let held =
            |unrestricted: &[Permission], scoped: Vec<(Permission, GrantScope)>| HeldAccess {
                unrestricted: unrestricted.iter().copied().collect::<PermissionSet>(),
                scoped,
            };
        assert!(configuration_scope(None, scope())
            .ok()
            .unwrap()
            .site_scope()
            .is_none());
        let everywhere = held(
            &[Permission::ContainersRead, Permission::ConfigRead],
            Vec::new(),
        );
        assert!(configuration_scope(Some(&everywhere), scope())
            .ok()
            .unwrap()
            .site_scope()
            .is_none());
        let containers_only = held(&[Permission::ContainersRead], Vec::new());
        assert!(configuration_scope(Some(&containers_only), scope()).is_err());
        let shop_only = held(
            &[Permission::ContainersRead],
            vec![(
                Permission::ConfigRead,
                GrantScope::SiteGroup {
                    group: "shop".into(),
                },
            )],
        );
        let limited = configuration_scope(Some(&shop_only), scope()).ok().unwrap();
        let site_scope = limited.site_scope().unwrap();
        assert!(!site_scope.everywhere("config.read"));
        assert!(site_scope.covers("config.read", "any-site", Some("shop")));
        assert!(!site_scope.covers("config.read", "any-site", Some("blog")));
    }
}
