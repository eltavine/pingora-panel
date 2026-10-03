//! Configuration resources: sites with their domains and routes, upstreams,
//! listeners and TLS profiles, plus applying the draft to the gateway.
//!
//! Each endpoint maps onto one named operation of the configuration port and
//! forwards the JSON body unchanged; the configuration service validates it.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    ActivatedDeployment, ApplyOutcome, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
    ConfigurationRead, DraftInfo, GatewayUseCases,
};
use panel_config_model::{
    BatchRequest, Domain, DomainCheck, DomainView, Listener, NodeInput, Route, RouteInput,
    RouteView, SiteBundle, SiteInput, SiteList, SiteQuery, SiteSummary, SiteView, UpstreamInput,
    UpstreamView, ValidationResult,
};
use panel_errors::PanelError;
use panel_ir::TlsProfile;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// The draft version a response was produced from.
pub(crate) static CONFIG_VERSION: HeaderName = HeaderName::from_static("x-config-version");
/// The draft version the gateway runs; absent until a draft is applied.
pub(crate) static CONFIG_APPLIED_VERSION: HeaderName =
    HeaderName::from_static("x-config-applied-version");

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn ConfigurationPort>, ApiError> {
    state.configuration.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "configuration management is not available here",
        ))
    })
}

fn respond(status: StatusCode, output: ConfigurationOutput) -> Response {
    let mut response = (status, output.content).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Some(etag) = output
        .etag
        .and_then(|etag| HeaderValue::from_str(&etag).ok())
    {
        headers.insert(header::ETAG, etag);
    }
    insert_draft(headers, &output.draft);
    response
}

fn insert_draft(headers: &mut HeaderMap, draft: &DraftInfo) {
    headers.insert(CONFIG_VERSION.clone(), HeaderValue::from(draft.version));
    if let Some(applied) = draft.applied_version {
        headers.insert(CONFIG_APPLIED_VERSION.clone(), HeaderValue::from(applied));
    }
    if let Some(updated) = draft.updated_at {
        headers.insert(
            header::LAST_MODIFIED,
            HeaderValue::from_str(&httpdate(updated)).expect("HTTP dates are visible ASCII"),
        );
    }
}

fn httpdate(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

async fn read<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    operation: &str,
    resource: String,
    parameters: Option<&impl Serialize>,
) -> Result<Response, ApiError> {
    let scope = request_scope(headers)?;
    let parameters = parameters
        .map(|parameters| serde_json::to_vec(parameters).expect("query parameters serialize"))
        .unwrap_or_default();
    let output = port(state)?
        .read(
            scope,
            ConfigurationRead {
                operation: operation.into(),
                resource,
                parameters,
            },
        )
        .await?;
    Ok(respond(StatusCode::OK, output))
}

/// How a change treats `If-Match`.
#[derive(Clone, Copy)]
enum Precondition {
    Optional,
    /// Replacing or deleting needs the representation the client last saw.
    Required,
}

async fn change<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    operation: &str,
    resource: String,
    body: Bytes,
    precondition: Precondition,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let context = command_context(headers)?;
    let if_match = headers
        .get(header::IF_MATCH)
        .map(|value| {
            value.to_str().map(str::to_owned).map_err(|_| {
                ApiError::new(PanelError::invalid_argument(
                    "If-Match must be visible ASCII",
                ))
            })
        })
        .transpose()?;
    if matches!(precondition, Precondition::Required) && if_match.is_none() {
        return Err(ApiError::new(PanelError::precondition_required(
            "send If-Match with the ETag of the representation being changed",
        )));
    }
    if !body.is_empty() {
        let json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
            });
        if !json {
            return Err(ApiError::new(PanelError::invalid_argument(
                "request bodies must be application/json",
            )));
        }
    }
    let output = port(state)?
        .change(
            context,
            ConfigurationChange {
                operation: operation.into(),
                resource,
                if_match,
                content: body.to_vec(),
            },
        )
        .await?;
    Ok(respond(status, output))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SitePath {
    /// Site identifier.
    id: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct DomainPath {
    id: Uuid,
    /// Host name in ASCII or Unicode form.
    host: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct NodePath {
    id: Uuid,
    node: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct NamedPath {
    /// Stable identifier chosen when the resource was created.
    id: String,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ExportQuery {
    /// Comma-separated site identifiers; all live sites when absent.
    ids: Option<String>,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteQuery {
    /// Removes the site for good instead of moving it to the recycle bin.
    #[serde(default)]
    permanent: bool,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DomainQuery {
    site_id: Option<Uuid>,
    /// Matches ASCII and Unicode host forms.
    q: Option<String>,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ValidationQuery {
    /// Comma-separated site identifiers; the whole draft when absent.
    site_ids: Option<String>,
}

fn id_list(value: Option<&str>) -> Result<Vec<Uuid>, ApiError> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Uuid::try_parse(value).map_err(|_| {
                ApiError::new(PanelError::invalid_argument(format!(
                    "{value:?} is not an id"
                )))
            })
        })
        .collect()
}

/// Body of `POST /sites/{id}/clone`.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct CloneSiteRequest {
    pub name: String,
}

/// Body of `PUT /sites/{id}/routes/order`.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct RouteOrderRequest {
    /// Every route of the site, first to last.
    pub order: Vec<Uuid>,
}

/// Body of `POST /domains/check`.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct DomainCheckRequest {
    pub hosts: Vec<String>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ImportResponse {
    pub created: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DraftResponse {
    pub version: u64,
    pub updated_at: Option<String>,
    pub applied_version: Option<u64>,
    pub applied_at: Option<String>,
    /// Whether the draft has changes the gateway does not run.
    pub pending: bool,
}

impl From<&DraftInfo> for DraftResponse {
    fn from(draft: &DraftInfo) -> Self {
        let time = |value: Option<std::time::SystemTime>| {
            value.map(|value| {
                DateTime::<Utc>::from(value).to_rfc3339_opts(SecondsFormat::Millis, true)
            })
        };
        Self {
            version: draft.version,
            updated_at: time(draft.updated_at),
            applied_version: draft.applied_version,
            applied_at: time(draft.applied_at),
            pending: draft.pending(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ApplyRequest {
    /// Refuses to apply if the draft changed since this version.
    #[serde(default)]
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ApplyResponse {
    pub draft: DraftResponse,
    pub revision_id: u64,
    pub content_hash: String,
    pub previous_active_hash: Option<String>,
}

impl ApplyResponse {
    fn new(draft: &DraftInfo, deployment: &ActivatedDeployment) -> Self {
        Self {
            draft: draft.into(),
            revision_id: deployment.revision_id().get(),
            content_hash: deployment.content_hash().as_str().into(),
            previous_active_hash: deployment
                .previous_active_hash()
                .map(|hash| hash.as_str().into()),
        }
    }
}

macro_rules! read_route {
    ($name:ident, $path:literal, $operation:literal, $resource:expr, $body:ty, $doc:literal) => {
        #[doc = $doc]
        #[utoipa::path(get, path = $path, params(QueryHeaders), responses((status = 200, body = $body)), tag = "configuration")]
        pub(crate) async fn $name<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
        ) -> Result<Response, ApiError> {
            read(&state, &headers, $operation, $resource.into(), None::<&()>).await
        }
    };
}

read_route!(
    list_upstreams,
    "/api/v1/upstreams",
    "upstreams.list",
    "upstreams",
    Vec<UpstreamView>,
    "Lists upstreams with the sites that use them."
);
read_route!(
    list_listeners,
    "/api/v1/listeners",
    "listeners.list",
    "listeners",
    Vec<Listener>,
    "Lists listeners."
);
read_route!(
    list_tls_profiles,
    "/api/v1/tls-profiles",
    "tls_profiles.list",
    "tls-profiles",
    Vec<TlsProfile>,
    "Lists TLS profiles."
);
read_route!(
    site_summary,
    "/api/v1/sites/summary",
    "sites.summary",
    "sites",
    SiteSummary,
    "Counts sites by status, type and HTTPS."
);

/// Lists sites matching the filters, one page at a time.
#[utoipa::path(get, path = "/api/v1/sites", params(QueryHeaders, SiteQuery), responses((status = 200, body = SiteList)), tag = "configuration")]
pub(crate) async fn list_sites<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<SiteQuery>,
) -> Result<Response, ApiError> {
    read(&state, &headers, "sites.list", "sites".into(), Some(&query)).await
}

/// Creates a site.
#[utoipa::path(post, path = "/api/v1/sites", request_body = SiteInput, params(MutationHeaders),
    responses((status = 201, body = SiteView, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn create_site<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "sites.create",
        "sites".into(),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Reads a site.
#[utoipa::path(get, path = "/api/v1/sites/{id}", params(QueryHeaders, SitePath),
    responses((status = 200, body = SiteView, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn get_site<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "sites.get",
        format!("sites/{}", path.id),
        None::<&()>,
    )
    .await
}

/// Replaces a site; `If-Match` must carry its current ETag.
#[utoipa::path(put, path = "/api/v1/sites/{id}", request_body = SiteInput,
    params(MutationHeaders, SitePath, ("If-Match" = String, Header, description = "ETag of the site being replaced")),
    responses((status = 200, body = SiteView)), tag = "configuration")]
pub(crate) async fn replace_site<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "sites.replace",
        format!("sites/{}", path.id),
        body,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Moves a site to the recycle bin, or removes it for good with `permanent`.
#[utoipa::path(delete, path = "/api/v1/sites/{id}",
    params(MutationHeaders, SitePath, DeleteQuery, ("If-Match" = String, Header, description = "ETag of the site")),
    responses((status = 200, body = SiteView)), tag = "configuration")]
pub(crate) async fn delete_site<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    Query(query): Query<DeleteQuery>,
) -> Result<Response, ApiError> {
    let operation = if query.permanent {
        "sites.purge"
    } else {
        "sites.delete"
    };
    change(
        &state,
        &headers,
        operation,
        format!("sites/{}", path.id),
        Bytes::new(),
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

macro_rules! site_action {
    ($name:ident, $path:literal, $operation:literal, $doc:literal) => {
        #[doc = $doc]
        #[utoipa::path(post, path = $path,
            params(MutationHeaders, SitePath, ("If-Match" = Option<String>, Header, description = "ETag the site must still have")),
            responses((status = 200, body = SiteView)), tag = "configuration")]
        pub(crate) async fn $name<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<SitePath>,
        ) -> Result<Response, ApiError> {
            change(&state, &headers, $operation, format!("sites/{}", path.id), Bytes::new(), Precondition::Optional, StatusCode::OK).await
        }
    };
}

site_action!(
    enable_site,
    "/api/v1/sites/{id}/enable",
    "sites.enable",
    "Starts serving a site."
);
site_action!(
    disable_site,
    "/api/v1/sites/{id}/disable",
    "sites.disable",
    "Stops serving a site while keeping its configuration."
);
site_action!(
    favorite_site,
    "/api/v1/sites/{id}/favorite",
    "sites.favorite",
    "Pins a site."
);
site_action!(
    unfavorite_site,
    "/api/v1/sites/{id}/unfavorite",
    "sites.unfavorite",
    "Unpins a site."
);
site_action!(
    restore_site,
    "/api/v1/sites/{id}/restore",
    "sites.restore",
    "Restores a site from the recycle bin."
);

/// Copies a site's settings and routes under a new name, without its domains.
#[utoipa::path(post, path = "/api/v1/sites/{id}/clone", request_body = CloneSiteRequest,
    params(MutationHeaders, SitePath), responses((status = 201, body = SiteView)), tag = "configuration")]
pub(crate) async fn clone_site<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "sites.clone",
        format!("sites/{}", path.id),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Applies one action to several sites; nothing changes unless every site can.
#[utoipa::path(post, path = "/api/v1/sites/batch", request_body = BatchRequest,
    params(MutationHeaders), responses((status = 200, body = Vec<SiteView>)), tag = "configuration")]
pub(crate) async fn batch_sites<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "sites.batch",
        "sites".into(),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Exports sites with the upstreams and TLS profiles they use.
#[utoipa::path(get, path = "/api/v1/sites/export", params(QueryHeaders, ExportQuery),
    responses((status = 200, body = SiteBundle)), tag = "configuration")]
pub(crate) async fn export_sites<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let ids = id_list(query.ids.as_deref())?;
    let mut response = read(
        &state,
        &headers,
        "sites.export",
        "sites".into(),
        Some(&serde_json::json!({ "ids": ids })),
    )
    .await?;
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"sites.json\""),
    );
    Ok(response)
}

/// Imports exported sites as new sites with new identities.
#[utoipa::path(post, path = "/api/v1/sites/import", request_body = SiteBundle,
    params(MutationHeaders), responses((status = 201, body = ImportResponse)), tag = "configuration")]
pub(crate) async fn import_sites<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "sites.import",
        "sites".into(),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Lists domains with their sites.
#[utoipa::path(get, path = "/api/v1/domains", params(QueryHeaders, DomainQuery),
    responses((status = 200, body = Vec<DomainView>)), tag = "configuration")]
pub(crate) async fn list_domains<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<DomainQuery>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "domains.list",
        "domains".into(),
        Some(&query),
    )
    .await
}

/// Checks host syntax, converts internationalized names and finds owners.
#[utoipa::path(post, path = "/api/v1/domains/check", request_body = DomainCheckRequest,
    params(QueryHeaders), responses((status = 200, body = Vec<DomainCheck>)), tag = "configuration")]
pub(crate) async fn check_domains<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request: serde_json::Value = serde_json::from_slice(&body).map_err(|error| {
        ApiError::new(PanelError::invalid_argument(format!(
            "invalid request body: {error}"
        )))
    })?;
    read(
        &state,
        &headers,
        "domains.check",
        "domains".into(),
        Some(&request),
    )
    .await
}

/// Binds domains to a site; one duplicate refuses the whole list.
#[utoipa::path(post, path = "/api/v1/sites/{id}/domains", request_body = Vec<Domain>,
    params(MutationHeaders, SitePath), responses((status = 200, body = SiteView)), tag = "configuration")]
pub(crate) async fn add_domains<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "domains.add",
        format!("sites/{}/domains", path.id),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Changes a domain's state, role or certificate.
#[utoipa::path(put, path = "/api/v1/sites/{id}/domains/{host}", request_body = Domain,
    params(MutationHeaders, DomainPath, ("If-Match" = String, Header, description = "ETag of the site")),
    responses((status = 200, body = SiteView)), tag = "configuration")]
pub(crate) async fn replace_domain<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<DomainPath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "domains.replace",
        format!("sites/{}/domains/{}", path.id, path.host),
        body,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Unbinds a domain from a site.
#[utoipa::path(delete, path = "/api/v1/sites/{id}/domains/{host}",
    params(MutationHeaders, DomainPath, ("If-Match" = String, Header, description = "ETag of the site")),
    responses((status = 200, body = SiteView)), tag = "configuration")]
pub(crate) async fn remove_domain<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<DomainPath>,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "domains.remove",
        format!("sites/{}/domains/{}", path.id, path.host),
        Bytes::new(),
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Lists a site's routes in evaluation order.
#[utoipa::path(get, path = "/api/v1/sites/{id}/routes", params(QueryHeaders, SitePath),
    responses((status = 200, body = Vec<Route>)), tag = "configuration")]
pub(crate) async fn list_routes<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "routes.list",
        format!("sites/{}/routes", path.id),
        None::<&()>,
    )
    .await
}

/// Adds a route to a site.
#[utoipa::path(post, path = "/api/v1/sites/{id}/routes", request_body = RouteInput,
    params(MutationHeaders, SitePath), responses((status = 201, body = RouteView)), tag = "configuration")]
pub(crate) async fn create_route<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "routes.create",
        format!("sites/{}/routes", path.id),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Reorders a site's routes, assigning priorities in the given order.
#[utoipa::path(put, path = "/api/v1/sites/{id}/routes/order", request_body = RouteOrderRequest,
    params(MutationHeaders, SitePath), responses((status = 200, body = Vec<Route>)), tag = "configuration")]
pub(crate) async fn reorder_routes<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "routes.reorder",
        format!("sites/{}/routes", path.id),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Reads a route.
#[utoipa::path(get, path = "/api/v1/routes/{id}", params(QueryHeaders, SitePath),
    responses((status = 200, body = RouteView, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn get_route<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "routes.get",
        format!("routes/{}", path.id),
        None::<&()>,
    )
    .await
}

/// Replaces a route.
#[utoipa::path(put, path = "/api/v1/routes/{id}", request_body = RouteInput,
    params(MutationHeaders, SitePath, ("If-Match" = String, Header, description = "ETag of the route")),
    responses((status = 200, body = RouteView)), tag = "configuration")]
pub(crate) async fn replace_route<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "routes.replace",
        format!("routes/{}", path.id),
        body,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Deletes a route.
#[utoipa::path(delete, path = "/api/v1/routes/{id}",
    params(MutationHeaders, SitePath, ("If-Match" = String, Header, description = "ETag of the route")),
    responses((status = 200)), tag = "configuration")]
pub(crate) async fn delete_route<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "routes.delete",
        format!("routes/{}", path.id),
        Bytes::new(),
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Creates an upstream.
#[utoipa::path(post, path = "/api/v1/upstreams", request_body = UpstreamInput,
    params(MutationHeaders), responses((status = 201, body = UpstreamView)), tag = "configuration")]
pub(crate) async fn create_upstream<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "upstreams.create",
        "upstreams".into(),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Reads an upstream.
#[utoipa::path(get, path = "/api/v1/upstreams/{id}", params(QueryHeaders, SitePath),
    responses((status = 200, body = UpstreamView, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn get_upstream<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "upstreams.get",
        format!("upstreams/{}", path.id),
        None::<&()>,
    )
    .await
}

/// Replaces an upstream; nodes keep their identity when their id is sent.
#[utoipa::path(put, path = "/api/v1/upstreams/{id}", request_body = UpstreamInput,
    params(MutationHeaders, SitePath, ("If-Match" = String, Header, description = "ETag of the upstream")),
    responses((status = 200, body = UpstreamView)), tag = "configuration")]
pub(crate) async fn replace_upstream<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "upstreams.replace",
        format!("upstreams/{}", path.id),
        body,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Deletes an upstream no site uses.
#[utoipa::path(delete, path = "/api/v1/upstreams/{id}",
    params(MutationHeaders, SitePath, ("If-Match" = String, Header, description = "ETag of the upstream")),
    responses((status = 200)), tag = "configuration")]
pub(crate) async fn delete_upstream<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "upstreams.delete",
        format!("upstreams/{}", path.id),
        Bytes::new(),
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Adds a node to an upstream.
#[utoipa::path(post, path = "/api/v1/upstreams/{id}/nodes", request_body = NodeInput,
    params(MutationHeaders, SitePath), responses((status = 201, body = UpstreamView)), tag = "configuration")]
pub(crate) async fn add_node<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "nodes.add",
        format!("upstreams/{}/nodes", path.id),
        body,
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Replaces a node: address, weight, state, role or note.
#[utoipa::path(put, path = "/api/v1/upstreams/{id}/nodes/{node}", request_body = NodeInput,
    params(MutationHeaders, NodePath, ("If-Match" = String, Header, description = "ETag of the upstream")),
    responses((status = 200, body = UpstreamView)), tag = "configuration")]
pub(crate) async fn replace_node<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<NodePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "nodes.replace",
        format!("upstreams/{}/nodes/{}", path.id, path.node),
        body,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Removes a node from an upstream.
#[utoipa::path(delete, path = "/api/v1/upstreams/{id}/nodes/{node}",
    params(MutationHeaders, NodePath, ("If-Match" = String, Header, description = "ETag of the upstream")),
    responses((status = 200, body = UpstreamView)), tag = "configuration")]
pub(crate) async fn delete_node<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<NodePath>,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "nodes.delete",
        format!("upstreams/{}/nodes/{}", path.id, path.node),
        Bytes::new(),
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

macro_rules! named_resource {
    ($get:ident, $put:ident, $delete:ident, $path:literal, $prefix:literal, $kind:literal, $body:ty) => {
        #[utoipa::path(get, path = $path, params(QueryHeaders, NamedPath),
            responses((status = 200, body = $body, headers(("ETag" = String)))), tag = "configuration")]
        pub(crate) async fn $get<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
        ) -> Result<Response, ApiError> {
            read(&state, &headers, concat!($kind, ".get"), format!(concat!($prefix, "/{}"), path.id), None::<&()>).await
        }

        /// Creates or replaces the resource; replacing needs `If-Match`.
        #[utoipa::path(put, path = $path, request_body = $body,
            params(MutationHeaders, NamedPath, ("If-Match" = Option<String>, Header, description = "ETag when replacing")),
            responses((status = 200, body = $body)), tag = "configuration")]
        pub(crate) async fn $put<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
            body: Bytes,
        ) -> Result<Response, ApiError> {
            change(&state, &headers, concat!($kind, ".put"), format!(concat!($prefix, "/{}"), path.id), body, Precondition::Optional, StatusCode::OK).await
        }

        #[utoipa::path(delete, path = $path,
            params(MutationHeaders, NamedPath, ("If-Match" = String, Header, description = "ETag of the resource")),
            responses((status = 200)), tag = "configuration")]
        pub(crate) async fn $delete<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
        ) -> Result<Response, ApiError> {
            change(&state, &headers, concat!($kind, ".delete"), format!(concat!($prefix, "/{}"), path.id), Bytes::new(), Precondition::Required, StatusCode::OK).await
        }
    };
}

named_resource!(
    get_listener,
    put_listener,
    delete_listener,
    "/api/v1/listeners/{id}",
    "listeners",
    "listeners",
    Listener
);
named_resource!(
    get_tls_profile,
    put_tls_profile,
    delete_tls_profile,
    "/api/v1/tls-profiles/{id}",
    "tls-profiles",
    "tls_profiles",
    TlsProfile
);

/// The draft's version and whether the gateway runs it.
#[utoipa::path(get, path = "/api/v1/config/draft", params(QueryHeaders),
    responses((status = 200, body = DraftResponse)), tag = "configuration")]
pub(crate) async fn draft<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = request_scope(&headers)?;
    let output = port(&state)?
        .read(
            scope,
            ConfigurationRead {
                operation: "sites.summary".into(),
                resource: "sites".into(),
                parameters: Vec::new(),
            },
        )
        .await?;
    let mut response = axum::Json(DraftResponse::from(&output.draft)).into_response();
    insert_draft(response.headers_mut(), &output.draft);
    Ok(response)
}

/// Validates the draft, or only the listed sites.
#[utoipa::path(get, path = "/api/v1/config/validation", params(QueryHeaders, ValidationQuery),
    responses((status = 200, body = ValidationResult)), tag = "configuration")]
pub(crate) async fn validation<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<ValidationQuery>,
) -> Result<Response, ApiError> {
    let site_ids = id_list(query.site_ids.as_deref())?;
    read(
        &state,
        &headers,
        "config.validate",
        String::new(),
        Some(&serde_json::json!({ "site_ids": site_ids })),
    )
    .await
}

/// Compiles the draft and activates it on the gateway.
#[utoipa::path(post, path = "/api/v1/config/apply", request_body = ApplyRequest,
    params(MutationHeaders), responses((status = 200, body = ApplyResponse)), tag = "configuration")]
pub(crate) async fn apply<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    let request: ApplyRequest = if body.is_empty() {
        ApplyRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|error| {
            ApiError::new(PanelError::invalid_argument(format!(
                "invalid request body: {error}"
            )))
        })?
    };
    match port(&state)?
        .apply(context, request.expected_version.unwrap_or(0))
        .await?
    {
        ApplyOutcome::Applied { draft, deployment } => {
            let mut response = axum::Json(ApplyResponse::new(&draft, &deployment)).into_response();
            insert_draft(response.headers_mut(), &draft);
            Ok(response)
        }
        ApplyOutcome::Rejected { report, .. } => Err(ApiError::new(
            PanelError::validation_failed("the draft is not valid; nothing was applied")
                .with_diagnostics(report.diagnostics),
        )),
        _ => Err(ApiError::new(PanelError::internal("unknown apply outcome"))),
    }
}
