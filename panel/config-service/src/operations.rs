//! The configuration API's operations over the draft model. Each operation
//! name is part of the contract that permissions and audit records use.

use chrono::{DateTime, Utc};
use panel_config_model::{
    abnormal_sites, checked, entity_tag, query_sites, summarize, validate, BatchAction,
    BatchRequest, ConfigModel, Domain, DomainCheck, DomainView, Listener, NodeInput, Route,
    RouteInput, RouteView, SiteBundle, SiteInput, SiteList, SiteQuery, SiteView, UpstreamInput,
    UpstreamView, ValidationResult,
};
use panel_domain::NormalizedHost;
use panel_errors::{Diagnostic, PanelError, Result};
use panel_ir::TlsProfile;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// A JSON result and, for single resources, its entity tag.
#[derive(Debug)]
pub struct Output {
    pub content: Vec<u8>,
    pub etag: String,
}

impl Output {
    fn json(value: &impl Serialize) -> Self {
        Self {
            content: serde_json::to_vec(value).expect("API values serialize"),
            etag: String::new(),
        }
    }

    fn tagged(value: &impl Serialize, etag: String) -> Self {
        Self {
            etag,
            ..Self::json(value)
        }
    }
}

enum Path<'a> {
    Root,
    Sites,
    Site(Uuid),
    SiteDomains(Uuid),
    SiteDomain(Uuid, NormalizedHost),
    SiteRoutes(Uuid),
    Route(Uuid),
    Upstreams,
    Upstream(Uuid),
    Nodes(Uuid),
    Node(Uuid, Uuid),
    Listeners,
    Listener(&'a str),
    TlsProfiles,
    TlsProfile(&'a str),
    Domains,
}

fn parse(resource: &str) -> Result<Path<'_>> {
    let id = |segment: &str| {
        Uuid::try_parse(segment)
            .map_err(|_| PanelError::invalid_argument(format!("{segment:?} is not an id")))
    };
    let segments: Vec<&str> = resource
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    Ok(match segments.as_slice() {
        [] => Path::Root,
        ["sites"] => Path::Sites,
        ["sites", site] => Path::Site(id(site)?),
        ["sites", site, "domains"] => Path::SiteDomains(id(site)?),
        ["sites", site, "domains", host] => Path::SiteDomain(
            id(site)?,
            NormalizedHost::new(host)
                .map_err(|error| PanelError::invalid_argument(error.to_string()))?,
        ),
        ["sites", site, "routes"] => Path::SiteRoutes(id(site)?),
        ["routes", route] => Path::Route(id(route)?),
        ["upstreams"] => Path::Upstreams,
        ["upstreams", upstream] => Path::Upstream(id(upstream)?),
        ["upstreams", upstream, "nodes"] => Path::Nodes(id(upstream)?),
        ["upstreams", upstream, "nodes", node] => Path::Node(id(upstream)?, id(node)?),
        ["listeners"] => Path::Listeners,
        ["listeners", listener] => Path::Listener(listener),
        ["tls-profiles"] => Path::TlsProfiles,
        ["tls-profiles", profile] => Path::TlsProfile(profile),
        ["domains"] => Path::Domains,
        _ => return Err(PanelError::not_found(format!("no resource {resource:?}"))),
    })
}

fn decode<T: DeserializeOwned>(content: &[u8]) -> Result<T> {
    let content = if content.is_empty() {
        b"null".as_slice()
    } else {
        content
    };
    serde_json::from_slice(content)
        .map_err(|error| PanelError::invalid_argument(format!("invalid request body: {error}")))
}

fn unsupported(operation: &str, resource: &str) -> PanelError {
    PanelError::invalid_argument(format!("{operation} does not apply to {resource:?}"))
}

/// RFC 9110 §13.1.1: `*` matches any current representation.
fn precondition(if_match: &str, current: &str) -> Result<()> {
    if if_match.is_empty()
        || if_match == "*"
        || if_match
            .split(',')
            .any(|candidate| candidate.trim() == current)
    {
        Ok(())
    } else {
        Err(PanelError::precondition_failed(
            "the resource changed since it was read; reload it and try again",
        ))
    }
}

fn site_view(model: &ConfigModel, id: Uuid) -> Result<Output> {
    let abnormal = abnormal_sites(model, &validate(model));
    let view = SiteView::new(model, model.site(id)?, &abnormal);
    let etag = view.etag.clone();
    Ok(Output::tagged(&view, etag))
}

fn route_view(model: &ConfigModel, id: Uuid) -> Result<Output> {
    let site = model.route_site(id)?;
    let route = site
        .routes
        .iter()
        .find(|route| route.id == id)
        .expect("route_site found the route")
        .clone();
    let etag = entity_tag(&route);
    Ok(Output::tagged(
        &RouteView {
            route,
            site_id: site.id,
            etag: etag.clone(),
        },
        etag,
    ))
}

fn upstream_view(model: &ConfigModel, id: Uuid) -> Result<Output> {
    let view = UpstreamView::new(model, model.upstream(id)?);
    let etag = view.etag.clone();
    Ok(Output::tagged(&view, etag))
}

fn named<'a, T: Serialize>(
    items: &'a [T],
    id: &str,
    key: impl Fn(&T) -> &str,
    kind: &str,
) -> Result<&'a T> {
    items
        .iter()
        .find(|item| key(item) == id)
        .ok_or_else(|| PanelError::not_found(format!("{kind} {id} does not exist")))
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ExportParameters {
    ids: Vec<Uuid>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DomainParameters {
    site_id: Option<Uuid>,
    q: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CheckParameters {
    hosts: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ValidateParameters {
    site_ids: Vec<Uuid>,
}

/// Answers a read operation against `model`.
pub fn read(
    model: &ConfigModel,
    operation: &str,
    resource: &str,
    parameters: &[u8],
) -> Result<Output> {
    let path = parse(resource)?;
    match (operation, path) {
        ("sites.list", Path::Sites) => {
            let query: SiteQuery = if parameters.is_empty() {
                SiteQuery::default()
            } else {
                decode(parameters)?
            };
            let abnormal = abnormal_sites(model, &validate(model));
            let page = query_sites(model, &abnormal, &query)?;
            Ok(Output::json(&SiteList {
                items: page
                    .items
                    .iter()
                    .map(|site| SiteView::new(model, site, &abnormal))
                    .collect(),
                next_cursor: page.next_cursor,
                total: page.total,
            }))
        }
        ("sites.get", Path::Site(id)) => site_view(model, id),
        ("sites.summary", Path::Sites) => {
            let abnormal = abnormal_sites(model, &validate(model));
            Ok(Output::json(&summarize(model, &abnormal)))
        }
        ("sites.export", Path::Sites) => {
            let parameters: ExportParameters = if parameters.is_empty() {
                ExportParameters::default()
            } else {
                decode(parameters)?
            };
            Ok(Output::json(&model.export_sites(&parameters.ids)?))
        }
        ("domains.list", Path::Domains) => {
            let parameters: DomainParameters = if parameters.is_empty() {
                DomainParameters::default()
            } else {
                decode(parameters)?
            };
            let keyword = parameters.q.map(|q| q.to_lowercase());
            let domains: Vec<DomainView> = DomainView::all(model)
                .into_iter()
                .filter(|view| parameters.site_id.is_none_or(|site| view.site_id == site))
                .filter(|view| {
                    keyword.as_ref().is_none_or(|keyword| {
                        view.domain.host.as_str().contains(keyword.as_str())
                            || view.unicode_host.contains(keyword.as_str())
                    })
                })
                .collect();
            Ok(Output::json(&domains))
        }
        ("domains.check", Path::Domains) => {
            let parameters: CheckParameters = decode(parameters)?;
            if parameters.hosts.len() > 1000 {
                return Err(PanelError::invalid_argument("at most 1000 hosts per check"));
            }
            let checks: Vec<DomainCheck> = parameters
                .hosts
                .iter()
                .map(|host| DomainCheck::new(model, host))
                .collect();
            Ok(Output::json(&checks))
        }
        ("routes.list", Path::SiteRoutes(site)) => {
            let mut routes: Vec<&Route> = model.site(site)?.routes.iter().collect();
            routes.sort_by_key(|route| (route.priority, route.id));
            Ok(Output::json(&routes))
        }
        ("routes.get", Path::Route(id)) => route_view(model, id),
        ("upstreams.list", Path::Upstreams) => {
            let views: Vec<UpstreamView> = model
                .upstreams
                .iter()
                .map(|upstream| UpstreamView::new(model, upstream))
                .collect();
            Ok(Output::json(&views))
        }
        ("upstreams.get", Path::Upstream(id)) => upstream_view(model, id),
        ("listeners.list", Path::Listeners) => Ok(Output::json(&model.listeners)),
        ("listeners.get", Path::Listener(id)) => {
            let listener = named(&model.listeners, id, |item| &item.id, "listener")?;
            Ok(Output::tagged(listener, entity_tag(listener)))
        }
        ("tls_profiles.list", Path::TlsProfiles) => Ok(Output::json(&model.tls_profiles)),
        ("tls_profiles.get", Path::TlsProfile(id)) => {
            let profile = named(&model.tls_profiles, id, |item| &item.id, "TLS profile")?;
            Ok(Output::tagged(profile, entity_tag(profile)))
        }
        ("config.validate", Path::Root) => {
            let parameters: ValidateParameters = if parameters.is_empty() {
                ValidateParameters::default()
            } else {
                decode(parameters)?
            };
            for site in &parameters.site_ids {
                model.site(*site)?;
            }
            let diagnostics: Vec<Diagnostic> = validate(model)
                .into_iter()
                .filter(|diagnostic| {
                    parameters.site_ids.is_empty()
                        || parameters.site_ids.iter().any(|site| {
                            diagnostic.resource_id.as_deref().is_some_and(|resource| {
                                resource.starts_with(&format!("sites/{site}"))
                            })
                        })
                })
                .collect();
            Ok(Output::json(&ValidationResult {
                valid: diagnostics.is_empty(),
                diagnostics,
            }))
        }
        (operation, _) => Err(unsupported(operation, resource)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloneRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReorderRequest {
    order: Vec<Uuid>,
}

#[derive(Serialize)]
struct Created {
    created: Vec<Uuid>,
}

/// Applies a change operation, returning the new model and its result.
pub fn change(
    model: &ConfigModel,
    operation: &str,
    resource: &str,
    if_match: &str,
    content: &[u8],
    now: DateTime<Utc>,
) -> Result<(ConfigModel, Output)> {
    let path = parse(resource)?;
    let site_tag = |id| model.site(id).map(entity_tag);
    let upstream_tag = |id| model.upstream(id).map(entity_tag);
    match (operation, path) {
        ("sites.create", Path::Sites) => {
            let input: SiteInput = decode(content)?;
            let (next, id) = checked(model, |model| Ok(model.create_site(input, now)))?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        ("sites.replace", Path::Site(id)) => {
            precondition(if_match, &site_tag(id)?)?;
            let input: SiteInput = decode(content)?;
            let (next, ()) = checked(model, |model| model.replace_site(id, input, now))?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        (
            "sites.enable" | "sites.disable" | "sites.favorite" | "sites.unfavorite"
            | "sites.delete" | "sites.restore",
            Path::Site(id),
        ) => {
            precondition(if_match, &site_tag(id)?)?;
            let (next, ()) = checked(model, |model| match operation {
                "sites.enable" => model.set_site_enabled(id, true, now),
                "sites.disable" => model.set_site_enabled(id, false, now),
                "sites.favorite" => model.set_site_favorite(id, true, now),
                "sites.unfavorite" => model.set_site_favorite(id, false, now),
                "sites.delete" => model.delete_site(id, now),
                _ => model.restore_site(id, now),
            })?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        ("sites.purge", Path::Site(id)) => {
            precondition(if_match, &site_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.purge_site(id))?;
            Ok((next, Output::json(&serde_json::json!({}))))
        }
        ("sites.clone", Path::Site(id)) => {
            let request: CloneRequest = decode(content)?;
            let (next, clone) = checked(model, |model| model.clone_site(id, request.name, now))?;
            let output = site_view(&next, clone)?;
            Ok((next, output))
        }
        ("sites.import", Path::Sites) => {
            let bundle: SiteBundle = decode(content)?;
            let (next, created) = checked(model, |model| model.import_sites(bundle, now))?;
            Ok((next, Output::json(&Created { created })))
        }
        ("sites.batch", Path::Sites) => {
            let request: BatchRequest = decode(content)?;
            let ids: BTreeSet<Uuid> = request.ids.iter().copied().collect();
            if ids.is_empty() || ids.len() != request.ids.len() {
                return Err(PanelError::invalid_argument("list each site exactly once"));
            }
            let (next, ()) = checked(model, |model| {
                for id in &ids {
                    match request.action {
                        BatchAction::Enable => model.set_site_enabled(*id, true, now)?,
                        BatchAction::Disable => model.set_site_enabled(*id, false, now)?,
                        BatchAction::Delete => model.delete_site(*id, now)?,
                        BatchAction::Restore => model.restore_site(*id, now)?,
                        BatchAction::Purge => model.purge_site(*id)?,
                        _ => return Err(PanelError::invalid_argument("unknown batch action")),
                    }
                }
                Ok(())
            })?;
            let abnormal = abnormal_sites(&next, &validate(&next));
            let views: Vec<SiteView> = ids
                .iter()
                .filter_map(|id| next.site(*id).ok())
                .map(|site| SiteView::new(&next, site, &abnormal))
                .collect();
            Ok((next, Output::json(&views)))
        }
        ("domains.add", Path::SiteDomains(site)) => {
            let domains: Vec<Domain> = decode(content)?;
            if domains.is_empty() || domains.len() > 1000 {
                return Err(PanelError::invalid_argument(
                    "add between 1 and 1000 domains",
                ));
            }
            let (next, ()) = checked(model, |model| model.add_domains(site, domains, now))?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ("domains.replace", Path::SiteDomain(site, host)) => {
            precondition(if_match, &site_tag(site)?)?;
            let domain: Domain = decode(content)?;
            let (next, ()) = checked(model, |model| {
                model.replace_domain(site, &host, domain, now)
            })?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ("domains.remove", Path::SiteDomain(site, host)) => {
            precondition(if_match, &site_tag(site)?)?;
            let (next, ()) = checked(model, |model| model.remove_domain(site, &host, now))?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ("routes.create", Path::SiteRoutes(site)) => {
            let input: RouteInput = decode(content)?;
            let (next, id) = checked(model, |model| model.create_route(site, input, now))?;
            let output = route_view(&next, id)?;
            Ok((next, output))
        }
        ("routes.reorder", Path::SiteRoutes(site)) => {
            let request: ReorderRequest = decode(content)?;
            let (next, ()) = checked(model, |model| {
                model.reorder_routes(site, &request.order, now)
            })?;
            let output = read(&next, "routes.list", resource, &[])?;
            Ok((next, output))
        }
        ("routes.replace" | "routes.delete", Path::Route(id)) => {
            precondition(if_match, &route_view(model, id)?.etag)?;
            if operation == "routes.delete" {
                let (next, ()) = checked(model, |model| model.delete_route(id, now))?;
                return Ok((next, Output::json(&serde_json::json!({}))));
            }
            let input: RouteInput = decode(content)?;
            let (next, ()) = checked(model, |model| model.replace_route(id, input, now))?;
            let output = route_view(&next, id)?;
            Ok((next, output))
        }
        ("upstreams.create", Path::Upstreams) => {
            let input: UpstreamInput = decode(content)?;
            let (next, id) = checked(model, |model| Ok(model.create_upstream(input, now)))?;
            let output = upstream_view(&next, id)?;
            Ok((next, output))
        }
        ("upstreams.replace", Path::Upstream(id)) => {
            precondition(if_match, &upstream_tag(id)?)?;
            let input: UpstreamInput = decode(content)?;
            let (next, ()) = checked(model, |model| model.replace_upstream(id, input, now))?;
            let output = upstream_view(&next, id)?;
            Ok((next, output))
        }
        ("upstreams.delete", Path::Upstream(id)) => {
            precondition(if_match, &upstream_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.delete_upstream(id))?;
            Ok((next, Output::json(&serde_json::json!({}))))
        }
        ("nodes.add", Path::Nodes(upstream)) => {
            let input: NodeInput = decode(content)?;
            let (next, _) = checked(model, |model| model.add_node(upstream, input, now))?;
            let output = upstream_view(&next, upstream)?;
            Ok((next, output))
        }
        ("nodes.replace" | "nodes.delete", Path::Node(upstream, node)) => {
            precondition(if_match, &upstream_tag(upstream)?)?;
            let (next, ()) = if operation == "nodes.delete" {
                checked(model, |model| model.delete_node(upstream, node, now))?
            } else {
                let input: NodeInput = decode(content)?;
                checked(model, |model| {
                    model.replace_node(upstream, node, input, now)
                })?
            };
            let output = upstream_view(&next, upstream)?;
            Ok((next, output))
        }
        ("listeners.put", Path::Listener(id)) => {
            let listener: Listener = decode(content)?;
            if listener.id != id {
                return Err(PanelError::invalid_argument(
                    "the body id must match the path",
                ));
            }
            if let Ok(existing) = named(&model.listeners, id, |item| &item.id, "listener") {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_listener(listener)))?;
            let listener = named(&next.listeners, id, |item| &item.id, "listener")?.clone();
            let etag = entity_tag(&listener);
            Ok((next, Output::tagged(&listener, etag)))
        }
        ("listeners.delete", Path::Listener(id)) => {
            precondition(
                if_match,
                &entity_tag(named(&model.listeners, id, |item| &item.id, "listener")?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_listener(id))?;
            Ok((next, Output::json(&serde_json::json!({}))))
        }
        ("tls_profiles.put", Path::TlsProfile(id)) => {
            let profile: TlsProfile = decode(content)?;
            if profile.id != id {
                return Err(PanelError::invalid_argument(
                    "the body id must match the path",
                ));
            }
            if let Ok(existing) = named(&model.tls_profiles, id, |item| &item.id, "TLS profile") {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_tls_profile(profile)))?;
            let profile = named(&next.tls_profiles, id, |item| &item.id, "TLS profile")?.clone();
            let etag = entity_tag(&profile);
            Ok((next, Output::tagged(&profile, etag)))
        }
        ("tls_profiles.delete", Path::TlsProfile(id)) => {
            precondition(
                if_match,
                &entity_tag(named(
                    &model.tls_profiles,
                    id,
                    |item| &item.id,
                    "TLS profile",
                )?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_tls_profile(id))?;
            Ok((next, Output::json(&serde_json::json!({}))))
        }
        (operation, _) => Err(unsupported(operation, resource)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn apply(
        model: &ConfigModel,
        operation: &str,
        resource: &str,
        body: Value,
    ) -> (ConfigModel, Value) {
        let (next, output) = change(
            model,
            operation,
            resource,
            "",
            &serde_json::to_vec(&body).unwrap(),
            Utc::now(),
        )
        .unwrap();
        (next, serde_json::from_slice(&output.content).unwrap())
    }

    fn get(model: &ConfigModel, operation: &str, resource: &str, parameters: Value) -> Value {
        let parameters = if parameters.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&parameters).unwrap()
        };
        serde_json::from_slice(
            &read(model, operation, resource, &parameters)
                .unwrap()
                .content,
        )
        .unwrap()
    }

    #[test]
    fn sites_upstreams_and_routes_round_trip_through_operations() {
        let model = ConfigModel::default();
        let (model, upstream) = apply(
            &model,
            "upstreams.create",
            "upstreams",
            json!({"name": "app", "nodes": [{"host": "127.0.0.1", "port": 8080}]}),
        );
        let upstream_id = upstream["id"].as_str().unwrap().to_owned();
        let (model, site) = apply(
            &model,
            "sites.create",
            "sites",
            json!({
                "name": "shop",
                "action": {"type": "proxy", "upstream_id": upstream_id},
                "domains": [{"host": "Shop.Example.com"}, {"host": "bücher.example"}]
            }),
        );
        assert_eq!(site["status"], "running");
        assert_eq!(site["kind"], "reverse_proxy");
        assert_eq!(
            site["unicode_hosts"]["xn--bcher-kva.example"],
            "bücher.example"
        );
        let site_id = site["id"].as_str().unwrap().to_owned();
        let (model, route) = apply(
            &model,
            "routes.create",
            &format!("sites/{site_id}/routes"),
            json!({"priority": 5, "match": {"kind": "prefix", "path": "/api"}, "action": {"type": "respond", "status": 204}}),
        );
        assert_eq!(route["site_id"], site_id);
        let list = get(&model, "sites.list", "sites", json!({"q": "shop"}));
        assert_eq!(list["total"], 1);
        let summary = get(&model, "sites.summary", "sites", Value::Null);
        assert_eq!(summary["reverse_proxy"], 1);
        let upstream = get(
            &model,
            "upstreams.get",
            &format!("upstreams/{upstream_id}"),
            Value::Null,
        );
        assert_eq!(upstream["used_by"][0], site_id);
        let checks = get(
            &model,
            "domains.check",
            "domains",
            json!({"hosts": ["SHOP.example.com", "bad host", "new.example"]}),
        );
        assert_eq!(checks[0]["owner"]["site_name"], "shop");
        assert!(checks[1]["error"].is_string());
        assert!(checks[2]["owner"].is_null());
        let error = change(
            &model,
            "upstreams.delete",
            &format!("upstreams/{upstream_id}"),
            "",
            &[],
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), panel_errors::ErrorCode::CONFLICT);
    }

    #[test]
    fn stale_entity_tags_and_unknown_operations_are_refused() {
        let model = ConfigModel::default();
        let (model, site) = apply(
            &model,
            "sites.create",
            "sites",
            json!({"name": "shop", "action": {"type": "respond"}}),
        );
        let resource = format!("sites/{}", site["id"].as_str().unwrap());
        let error = change(
            &model,
            "sites.disable",
            &resource,
            "\"stale\"",
            &[],
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::PRECONDITION_FAILED
        );
        let etag = site["etag"].as_str().unwrap();
        assert!(change(&model, "sites.disable", &resource, etag, &[], Utc::now()).is_ok());
        assert!(change(&model, "sites.explode", &resource, "", &[], Utc::now()).is_err());
        assert!(read(&model, "sites.get", "sites/not-an-id", &[]).is_err());
        assert_eq!(
            read(&model, "sites.get", "nowhere", &[])
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::NOT_FOUND
        );
    }

    #[test]
    fn batches_apply_entirely_or_not_at_all() {
        let model = ConfigModel::default();
        let (model, first) = apply(
            &model,
            "sites.create",
            "sites",
            json!({"name": "a", "action": {"type": "respond"}}),
        );
        let (model, second) = apply(
            &model,
            "sites.create",
            "sites",
            json!({"name": "b", "action": {"type": "respond"}}),
        );
        let ids = [first["id"].clone(), second["id"].clone()];
        let (model, views) = apply(
            &model,
            "sites.batch",
            "sites",
            json!({"action": "disable", "ids": ids}),
        );
        assert!(views
            .as_array()
            .unwrap()
            .iter()
            .all(|view| view["status"] == "stopped"));
        let error = change(
            &model,
            "sites.batch",
            "sites",
            "",
            &serde_json::to_vec(&json!({"action": "enable", "ids": [ids[0], Uuid::now_v7()]}))
                .unwrap(),
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), panel_errors::ErrorCode::NOT_FOUND);
        let unchanged = get(
            &model,
            "sites.get",
            &format!("sites/{}", ids[0].as_str().unwrap()),
            Value::Null,
        );
        assert_eq!(unchanged["status"], "stopped");
    }
}
