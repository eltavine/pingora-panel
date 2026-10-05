//! The route tester (ADR 0036): which site and route of the draft would
//! take a request, judged by the matcher the gateway serves with.

use crate::{
    configuration::{json, port},
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, Json};
use panel_config_api::LanguageQuery;
use panel_errors::PanelError;
use panel_ir::RuntimeSnapshot;
use panel_routing::{path, Router, SimulatedRequest};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::ToSchema;

/// A header field line of a simulated request.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HeaderLine {
    pub name: String,
    pub value: String,
}

/// A request to try against the draft's routes.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteTestRequest {
    /// Such as `GET`; `GET` when absent.
    #[serde(default)]
    pub method: Option<String>,
    /// The host the request names, with or without its port.
    pub host: String,
    /// The request target, such as `/api/items?tag=new`; `/` when absent.
    #[serde(default)]
    pub target: Option<String>,
    /// Header field lines in order; a repeated name is a line of its own.
    #[serde(default)]
    pub headers: Vec<HeaderLine>,
    /// The client's address, after trusted proxies.
    #[serde(default)]
    pub client: Option<String>,
    /// The listener the request arrives on, which may serve only some sites
    /// and name a default one.
    #[serde(default)]
    pub listener: Option<String>,
}

/// What became of a simulated request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RouteTestOutcome {
    /// A route takes it.
    Routed,
    /// No site serves its host here; the gateway answers 421.
    NoSite,
    /// The site has no route for it; the gateway answers 404.
    NoRoute,
}

/// A route the request was tried against, in the order they are tried.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RouteTrial {
    pub route_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub matched: bool,
    /// The first part that did not hold, for a route that did not match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Which site and route of the draft would take a request, and why.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RouteTestResult {
    pub outcome: RouteTestOutcome,
    /// The draft version tested.
    pub draft_version: u64,
    /// The host and path as routing compares them, normalized.
    pub host: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_id: Option<String>,
    /// Whether the site is the listener's default for hosts no site names.
    pub default_site: bool,
    /// The routes tried until one took the request.
    pub routes: Vec<RouteTrial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
}

/// The host as routing compares it: lowercase, without its port or a
/// trailing dot.
fn normalized_host(host: &str) -> String {
    let host = host.trim();
    let name = match host.strip_prefix('[') {
        Some(literal) => literal.split(']').next().unwrap_or(literal),
        None => match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => host,
        },
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// Which site and route of the draft would take a request: the routes
/// tried in order, each with the first part that did not hold, until one
/// takes it. The draft is compiled as an apply would, and judged by the
/// matcher the gateway serves requests with.
#[utoipa::path(post, path = "/api/v1/config/route-test", params(QueryHeaders),
    request_body = RouteTestRequest, responses((status = 200, body = RouteTestResult)),
    tag = "configuration")]
pub(crate) async fn test_route<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<RouteTestResult>, ApiError> {
    let wanted = json::<RouteTestRequest>(&headers, &body)?;
    let target = wanted.target.as_deref().unwrap_or("/");
    let (raw_path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query.to_owned())),
        None => (target, None),
    };
    let path = path::normalize(raw_path)
        .ok_or_else(|| {
            ApiError::new(PanelError::invalid_argument(
                "the target must be an absolute path, such as /api/items?tag=new",
            ))
        })?
        .into_owned();
    let client = wanted
        .client
        .as_deref()
        .map(|client| {
            client.trim().parse::<IpAddr>().map_err(|_| {
                ApiError::new(PanelError::invalid_argument(format!(
                    "{client:?} is not an IP address"
                )))
            })
        })
        .transpose()?;
    let request = SimulatedRequest {
        method: wanted.method.unwrap_or_else(|| "GET".to_owned()),
        host: normalized_host(&wanted.host),
        path,
        query,
        headers: wanted
            .headers
            .into_iter()
            .map(|line| (line.name.to_ascii_lowercase(), line.value))
            .collect(),
        client,
    };
    let output = port(&state)?
        .read(request_scope(&headers)?, LanguageQuery::Ir.into())
        .await?;
    let snapshot: RuntimeSnapshot = serde_json::from_slice(&output.content).map_err(|_| {
        ApiError::new(PanelError::corrupt_state(
            "the draft's runtime snapshot is unreadable",
        ))
    })?;
    let router = Router::compile(&snapshot).map_err(ApiError::new)?;
    let listener = wanted.listener.as_deref();
    let named = router
        .lookup(&request.host)
        .filter(|entry| listener.is_none_or(|listener| router.site(entry.site).serves(listener)))
        .map(|entry| entry.site);
    let fallback = listener.and_then(|listener| router.default_site(listener));
    let (site, default_site) = match (named, fallback) {
        (Some(site), _) => (Some(site), false),
        (None, Some(site)) => (Some(site), true),
        (None, None) => (None, false),
    };
    let mut result = RouteTestResult {
        outcome: RouteTestOutcome::NoSite,
        draft_version: output.draft.version,
        host: request.host.clone(),
        path: request.path.clone(),
        site_id: None,
        default_site,
        routes: Vec::new(),
        route_id: None,
    };
    let Some(site) = site.map(|site| router.site(site)) else {
        return Ok(Json(result));
    };
    result.site_id = Some(site.id.as_str().to_owned());
    result.outcome = RouteTestOutcome::NoRoute;
    for route in site.routes() {
        let mismatch = route.mismatch(&request);
        let matched = mismatch.is_none();
        result.routes.push(RouteTrial {
            route_id: route.id.as_str().to_owned(),
            name: route.name.clone(),
            matched,
            reason: mismatch.map(|mismatch| mismatch.to_string()),
        });
        if matched {
            result.outcome = RouteTestOutcome::Routed;
            result.route_id = Some(route.id.as_str().to_owned());
            break;
        }
    }
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::normalized_host;

    #[test]
    fn hosts_lose_their_ports_case_and_trailing_dots() {
        assert_eq!(normalized_host("Shop.Example:8443"), "shop.example");
        assert_eq!(normalized_host("shop.example."), "shop.example");
        assert_eq!(normalized_host("[2001:DB8::1]:443"), "2001:db8::1");
        assert_eq!(normalized_host("192.0.2.1"), "192.0.2.1");
    }
}
