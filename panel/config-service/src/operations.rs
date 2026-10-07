//! The configuration API's operations over the draft model.

use chrono::{DateTime, Utc};
use panel_config_api::{ModelChange, ModelQuery};
use panel_config_model::{
    abnormal_sites, checked, entity_tag, query_sites, summarize, validate, BatchAction,
    CachePolicyView, ConfigModel, DomainCheck, DomainView, HttpPolicyView, ListenerView, Route,
    RouteView, SecurityPolicyView, SiteList, SiteView, TlsProfile, TlsProfileView, UpstreamView,
    ValidationResult,
};
use panel_errors::{Diagnostic, PanelError, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

const POLICY: &str = "security policy";
const HTTP_POLICY: &str = "HTTP policy";
const CACHE_POLICY: &str = "cache policy";

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
        .expect("route_site found the route");
    let view = RouteView::new(site.id, route);
    let etag = view.etag.clone();
    Ok(Output::tagged(&view, etag))
}

fn routes(model: &ConfigModel, site: Uuid) -> Result<Output> {
    let site = model.site(site)?;
    let mut routes: Vec<&Route> = site.routes.iter().collect();
    routes.sort_by_key(|route| (route.priority, route.id));
    let views: Vec<RouteView> = routes
        .into_iter()
        .map(|route| RouteView::new(site.id, route))
        .collect();
    Ok(Output::json(&views))
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

#[derive(Serialize)]
struct Created {
    created: Vec<Uuid>,
}

/// Answers a read of `model`.
pub fn read(model: &ConfigModel, query: &ModelQuery) -> Result<Output> {
    match query {
        ModelQuery::Draft => Ok(Output::json(&serde_json::json!({}))),
        ModelQuery::Sites { query } => {
            let abnormal = abnormal_sites(model, &validate(model));
            let page = query_sites(model, &abnormal, query)?;
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
        ModelQuery::Site { id } => site_view(model, *id),
        ModelQuery::SiteSummary => {
            let abnormal = abnormal_sites(model, &validate(model));
            Ok(Output::json(&summarize(model, &abnormal)))
        }
        ModelQuery::ExportSites { ids } => Ok(Output::json(&model.export_sites(ids)?)),
        ModelQuery::Domains { site_id, q } => {
            let keyword = q.as_deref().map(str::to_lowercase);
            let domains: Vec<DomainView> = DomainView::all(model)
                .into_iter()
                .filter(|view| site_id.is_none_or(|site| view.site_id == site))
                .filter(|view| {
                    keyword.as_ref().is_none_or(|keyword| {
                        view.domain.host.as_str().contains(keyword.as_str())
                            || view.unicode_host.contains(keyword.as_str())
                    })
                })
                .collect();
            Ok(Output::json(&domains))
        }
        ModelQuery::CheckDomains { hosts } => {
            if hosts.len() > 1000 {
                return Err(PanelError::invalid_argument("at most 1000 hosts per check"));
            }
            let checks: Vec<DomainCheck> = hosts
                .iter()
                .map(|host| DomainCheck::new(model, host))
                .collect();
            Ok(Output::json(&checks))
        }
        ModelQuery::Routes { site } => routes(model, *site),
        ModelQuery::Route { id } => route_view(model, *id),
        ModelQuery::Upstreams => {
            let views: Vec<UpstreamView> = model
                .upstreams
                .iter()
                .map(|upstream| UpstreamView::new(model, upstream))
                .collect();
            Ok(Output::json(&views))
        }
        ModelQuery::Upstream { id } => upstream_view(model, *id),
        ModelQuery::Listeners => {
            let views: Vec<ListenerView> = model.listeners.iter().map(ListenerView::new).collect();
            Ok(Output::json(&views))
        }
        ModelQuery::Listener { id } => {
            let listener = named(&model.listeners, id, |item| &item.id, "listener")?;
            Ok(Output::tagged(listener, entity_tag(listener)))
        }
        ModelQuery::TlsProfiles => {
            let views: Vec<TlsProfileView> =
                model.tls_profiles.iter().map(TlsProfileView::new).collect();
            Ok(Output::json(&views))
        }
        ModelQuery::TlsProfile { id } => {
            let profile = named(&model.tls_profiles, id, |item| &item.id, "TLS profile")?;
            Ok(Output::tagged(profile, entity_tag(profile)))
        }
        ModelQuery::SecurityPolicies => {
            let views: Vec<SecurityPolicyView> = model
                .security_policies
                .iter()
                .map(|policy| SecurityPolicyView::new(model, policy))
                .collect();
            Ok(Output::json(&views))
        }
        ModelQuery::SecurityPolicy { id } => {
            let policy = named(&model.security_policies, id, |item| &item.id, POLICY)?;
            Ok(Output::tagged(policy, entity_tag(policy)))
        }
        ModelQuery::HttpPolicies => {
            let views: Vec<HttpPolicyView> = model
                .http_policies
                .iter()
                .map(|policy| HttpPolicyView::new(model, policy))
                .collect();
            Ok(Output::json(&views))
        }
        ModelQuery::HttpPolicy { id } => {
            let policy = named(&model.http_policies, id, |item| &item.id, HTTP_POLICY)?;
            Ok(Output::tagged(policy, entity_tag(policy)))
        }
        ModelQuery::CachePolicies => {
            let views: Vec<CachePolicyView> = model
                .cache_policies
                .iter()
                .map(|policy| CachePolicyView::new(model, policy))
                .collect();
            Ok(Output::json(&views))
        }
        ModelQuery::CachePolicy { id } => {
            let policy = named(&model.cache_policies, id, |item| &item.id, CACHE_POLICY)?;
            Ok(Output::tagged(policy, entity_tag(policy)))
        }
        ModelQuery::CacheSettings => Ok(Output::tagged(&model.cache, entity_tag(&model.cache))),
        ModelQuery::Validate { site_ids } => {
            for site in site_ids {
                model.site(*site)?;
            }
            let diagnostics: Vec<Diagnostic> = validate(model)
                .into_iter()
                .filter(|diagnostic| {
                    site_ids.is_empty()
                        || site_ids.iter().any(|site| {
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
    }
}

/// Applies `change` to `model`, returning the new model and its result.
/// `if_match` holds the entity tags the target must still match; empty
/// skips the check.
pub fn change(
    model: &ConfigModel,
    change: ModelChange,
    if_match: &str,
    now: DateTime<Utc>,
) -> Result<(ConfigModel, Output)> {
    let site_tag = |id| model.site(id).map(entity_tag);
    let upstream_tag = |id| model.upstream(id).map(entity_tag);
    let deleted = || Output::json(&serde_json::json!({}));
    match change {
        ModelChange::CreateSite { site } => {
            let (next, id) = checked(model, |model| Ok(model.create_site(site, now)))?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::ReplaceSite { id, site } => {
            precondition(if_match, &site_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.replace_site(id, site, now))?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::EnableSite { id }
        | ModelChange::DisableSite { id }
        | ModelChange::FavoriteSite { id }
        | ModelChange::UnfavoriteSite { id }
        | ModelChange::DeleteSite { id }
        | ModelChange::RestoreSite { id } => {
            precondition(if_match, &site_tag(id)?)?;
            let (next, ()) = checked(model, |model| match change {
                ModelChange::EnableSite { .. } => model.set_site_enabled(id, true, now),
                ModelChange::DisableSite { .. } => model.set_site_enabled(id, false, now),
                ModelChange::FavoriteSite { .. } => model.set_site_favorite(id, true, now),
                ModelChange::UnfavoriteSite { .. } => model.set_site_favorite(id, false, now),
                ModelChange::DeleteSite { .. } => model.delete_site(id, now),
                _ => model.restore_site(id, now),
            })?;
            let output = site_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::PurgeSite { id } => {
            precondition(if_match, &site_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.purge_site(id))?;
            Ok((next, deleted()))
        }
        ModelChange::CloneSite { id, name } => {
            let (next, clone) = checked(model, |model| model.clone_site(id, name, now))?;
            let output = site_view(&next, clone)?;
            Ok((next, output))
        }
        ModelChange::ImportSites { bundle } => {
            let (next, created) = checked(model, |model| model.import_sites(bundle, now))?;
            Ok((next, Output::json(&Created { created })))
        }
        ModelChange::BatchSites { batch } => {
            let ids: BTreeSet<Uuid> = batch.ids.iter().copied().collect();
            if ids.is_empty() || ids.len() != batch.ids.len() {
                return Err(PanelError::invalid_argument("list each site exactly once"));
            }
            let (next, ()) = checked(model, |model| {
                for id in &ids {
                    match batch.action {
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
        ModelChange::AddDomains { site, domains } => {
            if domains.is_empty() || domains.len() > 1000 {
                return Err(PanelError::invalid_argument(
                    "add between 1 and 1000 domains",
                ));
            }
            let (next, ()) = checked(model, |model| model.add_domains(site, domains, now))?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ModelChange::ReplaceDomain { site, host, domain } => {
            precondition(if_match, &site_tag(site)?)?;
            let (next, ()) = checked(model, |model| {
                model.replace_domain(site, &host, domain, now)
            })?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ModelChange::RemoveDomain { site, host } => {
            precondition(if_match, &site_tag(site)?)?;
            let (next, ()) = checked(model, |model| model.remove_domain(site, &host, now))?;
            let output = site_view(&next, site)?;
            Ok((next, output))
        }
        ModelChange::CreateRoute { site, route } => {
            let (next, id) = checked(model, |model| model.create_route(site, route, now))?;
            let output = route_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::ReorderRoutes { site, order } => {
            let (next, ()) = checked(model, |model| model.reorder_routes(site, &order, now))?;
            let output = routes(&next, site)?;
            Ok((next, output))
        }
        ModelChange::ReplaceRoute { id, route } => {
            precondition(if_match, &route_view(model, id)?.etag)?;
            let (next, ()) = checked(model, |model| model.replace_route(id, route, now))?;
            let output = route_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::DeleteRoute { id } => {
            precondition(if_match, &route_view(model, id)?.etag)?;
            let (next, ()) = checked(model, |model| model.delete_route(id, now))?;
            Ok((next, deleted()))
        }
        ModelChange::CreateUpstream { upstream } => {
            let (next, id) = checked(model, |model| Ok(model.create_upstream(upstream, now)))?;
            let output = upstream_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::ReplaceUpstream { id, upstream } => {
            precondition(if_match, &upstream_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.replace_upstream(id, upstream, now))?;
            let output = upstream_view(&next, id)?;
            Ok((next, output))
        }
        ModelChange::DeleteUpstream { id } => {
            precondition(if_match, &upstream_tag(id)?)?;
            let (next, ()) = checked(model, |model| model.delete_upstream(id))?;
            Ok((next, deleted()))
        }
        ModelChange::AddNode { upstream, node } => {
            let (next, _) = checked(model, |model| model.add_node(upstream, node, now))?;
            let output = upstream_view(&next, upstream)?;
            Ok((next, output))
        }
        ModelChange::ReplaceNode { upstream, id, node } => {
            precondition(if_match, &upstream_tag(upstream)?)?;
            let (next, ()) = checked(model, |model| model.replace_node(upstream, id, node, now))?;
            let output = upstream_view(&next, upstream)?;
            Ok((next, output))
        }
        ModelChange::DeleteNode { upstream, id } => {
            precondition(if_match, &upstream_tag(upstream)?)?;
            let (next, ()) = checked(model, |model| model.delete_node(upstream, id, now))?;
            let output = upstream_view(&next, upstream)?;
            Ok((next, output))
        }
        ModelChange::PutListener { listener } => {
            let id = listener.id.clone();
            if let Ok(existing) = named(&model.listeners, &id, |item| &item.id, "listener") {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_listener(listener)))?;
            let listener = named(&next.listeners, &id, |item| &item.id, "listener")?.clone();
            let etag = entity_tag(&listener);
            Ok((next, Output::tagged(&listener, etag)))
        }
        ModelChange::DeleteListener { id } => {
            precondition(
                if_match,
                &entity_tag(named(&model.listeners, &id, |item| &item.id, "listener")?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_listener(&id))?;
            Ok((next, deleted()))
        }
        ModelChange::PutTlsProfile { profile } => {
            let profile = TlsProfile::from(profile);
            let id = profile.id.clone();
            if let Ok(existing) = named(&model.tls_profiles, &id, |item| &item.id, "TLS profile") {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_tls_profile(profile)))?;
            let profile = named(&next.tls_profiles, &id, |item| &item.id, "TLS profile")?.clone();
            let etag = entity_tag(&profile);
            Ok((next, Output::tagged(&profile, etag)))
        }
        ModelChange::DeleteTlsProfile { id } => {
            precondition(
                if_match,
                &entity_tag(named(
                    &model.tls_profiles,
                    &id,
                    |item| &item.id,
                    "TLS profile",
                )?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_tls_profile(&id))?;
            Ok((next, deleted()))
        }
        ModelChange::PutSecurityPolicy { policy } => {
            let id = policy.id.clone();
            if let Ok(existing) = named(&model.security_policies, &id, |item| &item.id, POLICY) {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_security_policy(policy)))?;
            let policy = named(&next.security_policies, &id, |item| &item.id, POLICY)?.clone();
            let etag = entity_tag(&policy);
            Ok((next, Output::tagged(&policy, etag)))
        }
        ModelChange::DeleteSecurityPolicy { id } => {
            precondition(
                if_match,
                &entity_tag(named(
                    &model.security_policies,
                    &id,
                    |item| &item.id,
                    POLICY,
                )?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_security_policy(&id))?;
            Ok((next, deleted()))
        }
        ModelChange::PutHttpPolicy { policy } => {
            let id = policy.id.clone();
            if let Ok(existing) = named(&model.http_policies, &id, |item| &item.id, HTTP_POLICY) {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_http_policy(policy)))?;
            let policy = named(&next.http_policies, &id, |item| &item.id, HTTP_POLICY)?.clone();
            let etag = entity_tag(&policy);
            Ok((next, Output::tagged(&policy, etag)))
        }
        ModelChange::DeleteHttpPolicy { id } => {
            precondition(
                if_match,
                &entity_tag(named(
                    &model.http_policies,
                    &id,
                    |item| &item.id,
                    HTTP_POLICY,
                )?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_http_policy(&id))?;
            Ok((next, deleted()))
        }
        ModelChange::PutCachePolicy { policy } => {
            let id = policy.id.clone();
            if let Ok(existing) = named(&model.cache_policies, &id, |item| &item.id, CACHE_POLICY) {
                precondition(if_match, &entity_tag(existing))?;
            }
            let (next, _) = checked(model, |model| Ok(model.put_cache_policy(policy)))?;
            let policy = named(&next.cache_policies, &id, |item| &item.id, CACHE_POLICY)?.clone();
            let etag = entity_tag(&policy);
            Ok((next, Output::tagged(&policy, etag)))
        }
        ModelChange::DeleteCachePolicy { id } => {
            precondition(
                if_match,
                &entity_tag(named(
                    &model.cache_policies,
                    &id,
                    |item| &item.id,
                    CACHE_POLICY,
                )?),
            )?;
            let (next, ()) = checked(model, |model| model.delete_cache_policy(&id))?;
            Ok((next, deleted()))
        }
        ModelChange::PutCacheSettings { settings } => {
            precondition(if_match, &entity_tag(&model.cache))?;
            let (next, ()) = checked(model, |model| {
                model.cache = settings;
                Ok(())
            })?;
            let output = Output::tagged(&next.cache, entity_tag(&next.cache));
            Ok((next, output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_config_model::{BatchRequest, SiteQuery};
    use panel_errors::ErrorCode;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    /// A model input, written as JSON.
    fn input<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    fn apply(model: &ConfigModel, change: ModelChange) -> (ConfigModel, Value) {
        let (next, output) = super::change(model, change, "", Utc::now()).unwrap();
        (next, serde_json::from_slice(&output.content).unwrap())
    }

    fn get(model: &ConfigModel, query: ModelQuery) -> Value {
        serde_json::from_slice(&read(model, &query).unwrap().content).unwrap()
    }

    fn uuid(value: &Value) -> Uuid {
        value.as_str().unwrap().parse().unwrap()
    }

    #[test]
    fn security_policies_are_named_resources_that_sites_use() {
        let model = ConfigModel::default();
        let (model, policy) = apply(
            &model,
            ModelChange::PutSecurityPolicy {
                policy: input(json!({
                    "id": "office", "allowed_cidrs": ["10.0.0.0/8"], "allowed_methods": ["GET"]
                })),
            },
        );
        assert_eq!(policy["allowed_methods"][0], "GET");
        let (model, site) = apply(
            &model,
            ModelChange::CreateSite {
                site: input(json!({
                    "name": "intranet",
                    "action": {"type": "respond", "status": 200},
                    "domains": [{"host": "intranet.example"}],
                    "security_policy_id": "office",
                    "routes": [{"priority": 1, "match": {"kind": "prefix", "path": "/hr"},
                        "action": {"type": "respond", "status": 204}, "security_policy_id": "office"}]
                })),
            },
        );
        let site_id = site["id"].as_str().unwrap().to_owned();
        let list = get(&model, ModelQuery::SecurityPolicies);
        assert_eq!(list[0]["used_by"][0], site_id);
        assert!(list[0]["etag"].is_string());

        let office = || ModelQuery::SecurityPolicy {
            id: "office".into(),
        };
        let current = read(&model, &office()).unwrap();
        let put = |if_match: &str, policy: Value| {
            super::change(
                &model,
                ModelChange::PutSecurityPolicy {
                    policy: input(policy),
                },
                if_match,
                Utc::now(),
            )
        };
        let replaced = put("\"stale\"", json!({"id": "office"}));
        assert_eq!(
            replaced.unwrap_err().code.as_str(),
            ErrorCode::PRECONDITION_FAILED
        );
        let refused = super::change(
            &model,
            ModelChange::DeleteSecurityPolicy {
                id: "office".into(),
            },
            &current.etag,
            Utc::now(),
        );
        assert_eq!(refused.unwrap_err().code.as_str(), ErrorCode::CONFLICT);
        let invalid = put(
            &current.etag,
            json!({"id": "office", "rate_limits": [{"key": {"kind": "client_address"}, "requests": 0, "per_seconds": 1}]}),
        );
        assert_eq!(
            invalid.unwrap_err().code.as_str(),
            ErrorCode::VALIDATION_FAILED
        );
        let (model, _) = apply(
            &model,
            ModelChange::PutSecurityPolicy {
                policy: input(json!({"id": "open"})),
            },
        );
        assert_eq!(
            get(&model, ModelQuery::SecurityPolicies)
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn cache_policies_and_the_store_are_named_resources() {
        let model = ConfigModel::default();
        let (model, policy) = apply(
            &model,
            ModelChange::PutCachePolicy {
                policy: input(json!({
                    "id": "pages", "ttl_seconds": 600, "status_ttls": {"404": 60},
                    "bypass": [{"kind": "cookie", "name": "session", "test": {"op": "present"}}]
                })),
            },
        );
        assert_eq!(policy["status_ttls"]["404"], 60);
        let (model, site) = apply(
            &model,
            ModelChange::CreateSite {
                site: input(json!({
                    "name": "shop",
                    "action": {"type": "respond", "status": 200},
                    "domains": [{"host": "shop.example"}],
                    "cache_policy_id": "pages",
                    "routes": [{"priority": 1, "match": {"kind": "prefix", "path": "/live"},
                        "action": {"type": "respond", "status": 204}, "no_cache": true}]
                })),
            },
        );
        let list = get(&model, ModelQuery::CachePolicies);
        assert_eq!(list[0]["used_by"][0], site["id"]);
        let current = read(&model, &ModelQuery::CachePolicy { id: "pages".into() }).unwrap();
        let refused = super::change(
            &model,
            ModelChange::DeleteCachePolicy { id: "pages".into() },
            &current.etag,
            Utc::now(),
        );
        assert_eq!(refused.unwrap_err().code.as_str(), ErrorCode::CONFLICT);
        let invalid = super::change(
            &model,
            ModelChange::PutCachePolicy {
                policy: input(json!({"id": "pages", "key": ""})),
            },
            &current.etag,
            Utc::now(),
        );
        assert_eq!(
            invalid.unwrap_err().code.as_str(),
            ErrorCode::VALIDATION_FAILED
        );

        let store = read(&model, &ModelQuery::CacheSettings).unwrap();
        let resize = |if_match: &str, settings: Value| {
            super::change(
                &model,
                ModelChange::PutCacheSettings {
                    settings: input(settings),
                },
                if_match,
                Utc::now(),
            )
        };
        assert_eq!(
            resize("\"stale\"", json!({"max_bytes": 1_073_741_824}))
                .unwrap_err()
                .code
                .as_str(),
            ErrorCode::PRECONDITION_FAILED
        );
        assert_eq!(
            resize(&store.etag, json!({"max_bytes": 1024}))
                .unwrap_err()
                .code
                .as_str(),
            ErrorCode::VALIDATION_FAILED
        );
        let (resized, _) = resize(&store.etag, json!({"max_bytes": 1_073_741_824})).unwrap();
        assert_eq!(resized.cache.max_bytes, Some(1 << 30));
        assert_eq!(
            get(&resized, ModelQuery::CacheSettings)["max_bytes"],
            1_073_741_824
        );
    }

    #[test]
    fn sites_upstreams_and_routes_round_trip_through_operations() {
        let model = ConfigModel::default();
        let (model, upstream) = apply(
            &model,
            ModelChange::CreateUpstream {
                upstream: input(
                    json!({"name": "app", "nodes": [{"host": "127.0.0.1", "port": 8080}]}),
                ),
            },
        );
        let upstream_id = uuid(&upstream["id"]);
        let (model, site) = apply(
            &model,
            ModelChange::CreateSite {
                site: input(json!({
                    "name": "shop",
                    "action": {"type": "proxy", "upstream_id": upstream_id},
                    "domains": [{"host": "Shop.Example.com"}, {"host": "bücher.example"}]
                })),
            },
        );
        assert_eq!(site["status"], "running");
        assert_eq!(site["kind"], "reverse_proxy");
        assert_eq!(
            site["unicode_hosts"]["xn--bcher-kva.example"],
            "bücher.example"
        );
        let site_id = uuid(&site["id"]);
        let (model, route) = apply(
            &model,
            ModelChange::CreateRoute {
                site: site_id,
                route: input(
                    json!({"priority": 5, "match": {"kind": "prefix", "path": "/api"}, "action": {"type": "respond", "status": 204}}),
                ),
            },
        );
        assert_eq!(route["site_id"], site_id.to_string());
        let list = get(
            &model,
            ModelQuery::Sites {
                query: SiteQuery {
                    q: Some("shop".into()),
                    ..SiteQuery::default()
                },
            },
        );
        assert_eq!(list["total"], 1);
        let summary = get(&model, ModelQuery::SiteSummary);
        assert_eq!(summary["reverse_proxy"], 1);
        let upstream = get(&model, ModelQuery::Upstream { id: upstream_id });
        assert_eq!(upstream["used_by"][0], site_id.to_string());
        let checks = get(
            &model,
            ModelQuery::CheckDomains {
                hosts: vec![
                    "SHOP.example.com".into(),
                    "bad host".into(),
                    "new.example".into(),
                ],
            },
        );
        assert_eq!(checks[0]["owner"]["site_name"], "shop");
        assert!(checks[1]["error"].is_string());
        assert!(checks[2]["owner"].is_null());
        let error = super::change(
            &model,
            ModelChange::DeleteUpstream { id: upstream_id },
            "",
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::CONFLICT);
    }

    #[test]
    fn stale_entity_tags_and_missing_resources_are_refused() {
        let model = ConfigModel::default();
        let (model, site) = apply(
            &model,
            ModelChange::CreateSite {
                site: input(json!({"name": "shop", "action": {"type": "respond"}})),
            },
        );
        let id = uuid(&site["id"]);
        let disable = |if_match: &str| {
            super::change(
                &model,
                ModelChange::DisableSite { id },
                if_match,
                Utc::now(),
            )
        };
        assert_eq!(
            disable("\"stale\"").unwrap_err().code.as_str(),
            ErrorCode::PRECONDITION_FAILED
        );
        assert!(disable(site["etag"].as_str().unwrap()).is_ok());
        assert_eq!(
            read(&model, &ModelQuery::Site { id: Uuid::now_v7() })
                .unwrap_err()
                .code
                .as_str(),
            ErrorCode::NOT_FOUND
        );
    }

    #[test]
    fn batches_apply_entirely_or_not_at_all() {
        let model = ConfigModel::default();
        let create = |model: &ConfigModel, name: &str| {
            apply(
                model,
                ModelChange::CreateSite {
                    site: input(json!({"name": name, "action": {"type": "respond"}})),
                },
            )
        };
        let (model, first) = create(&model, "a");
        let (model, second) = create(&model, "b");
        let ids = [uuid(&first["id"]), uuid(&second["id"])];
        let batch = |action: &str, ids: &[Uuid]| ModelChange::BatchSites {
            batch: input::<BatchRequest>(json!({"action": action, "ids": ids})),
        };
        let (model, views) = apply(&model, batch("disable", &ids));
        assert!(views
            .as_array()
            .unwrap()
            .iter()
            .all(|view| view["status"] == "stopped"));
        let error = super::change(
            &model,
            batch("enable", &[ids[0], Uuid::now_v7()]),
            "",
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::NOT_FOUND);
        let unchanged = get(&model, ModelQuery::Site { id: ids[0] });
        assert_eq!(unchanged["status"], "stopped");
    }
}
