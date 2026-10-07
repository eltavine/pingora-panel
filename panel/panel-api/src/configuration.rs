//! Configuration resources: sites with their domains and routes, upstreams,
//! listeners and TLS profiles, plus applying the draft to the gateway.
//!
//! Each endpoint reads its request into one typed operation of the
//! configuration port; the configuration service validates what it asks.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{ActivatedDeployment, GatewayUseCases};
use panel_config_api::{
    ApplyOutcome, ApprovalBypass, ConfigurationChange, ConfigurationCommand, ConfigurationOutput,
    ConfigurationPort, ConfigurationQuery, DraftInfo, ModelChange, ModelQuery,
};
use panel_config_model::{
    ApprovalRequest, BatchRequest, CachePolicy, CachePolicyView, CacheSettings, Domain,
    DomainCheck, DomainView, HttpPolicy, HttpPolicyView, Listener, ListenerView, NodeInput,
    RouteInput, RouteView, SecurityPolicy, SecurityPolicyView, SiteBundle, SiteInput, SiteList,
    SiteQuery, SiteSummary, SiteView, TlsProfile, TlsProfileInput, TlsProfileView, UpstreamInput,
    UpstreamView, ValidationResult,
};
use panel_domain::NormalizedHost;
use panel_errors::PanelError;
use panel_identity::{Permission, Principal};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// The draft version a response was produced from.
pub(crate) static CONFIG_VERSION: HeaderName = HeaderName::from_static("x-config-version");
/// The draft version the gateway runs; absent until a draft is applied.
pub(crate) static CONFIG_APPLIED_VERSION: HeaderName =
    HeaderName::from_static("x-config-applied-version");

pub(crate) fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn ConfigurationPort>, ApiError> {
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

pub(crate) fn insert_draft(headers: &mut HeaderMap, draft: &DraftInfo) {
    headers.insert(CONFIG_VERSION.clone(), HeaderValue::from(draft.version));
    if let Some(applied) = draft.applied_version {
        headers.insert(CONFIG_APPLIED_VERSION.clone(), HeaderValue::from(applied));
    }
    if let Some(updated) = draft.updated_at {
        headers.insert(
            header::LAST_MODIFIED,
            HeaderValue::from_str(&httpdate::fmt_http_date(updated))
                .expect("HTTP dates are visible ASCII"),
        );
    }
}

pub(crate) async fn read<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    query: impl Into<ConfigurationQuery>,
) -> Result<Response, ApiError> {
    let scope = request_scope(headers)?;
    let output = port(state)?.read(scope, query.into()).await?;
    Ok(respond(StatusCode::OK, output))
}

/// A JSON request body as `T`.
pub(crate) fn json<T: DeserializeOwned>(headers: &HeaderMap, body: &Bytes) -> Result<T, ApiError> {
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
    serde_json::from_slice(body).map_err(|error| {
        ApiError::new(PanelError::invalid_argument(format!(
            "invalid request body: {error}"
        )))
    })
}

/// A host name from a path, in its normalized form.
fn host(value: &str) -> Result<NormalizedHost, ApiError> {
    NormalizedHost::new(value)
        .map_err(|error| ApiError::new(PanelError::invalid_argument(error.to_string())))
}

/// How a change treats `If-Match`.
#[derive(Clone, Copy)]
pub(crate) enum Precondition {
    Optional,
    /// Replacing or deleting needs the representation the client last saw.
    Required,
}

pub(crate) async fn change<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    command: impl Into<ConfigurationCommand>,
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
    let output = port(state)?
        .change(
            context,
            ConfigurationChange {
                command: command.into(),
                if_match,
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
    /// Recorded with the revision, for example why it was applied.
    #[serde(default)]
    pub note: Option<String>,
    /// Applies without the approvals policies ask for; needs
    /// `approval.bypass` and is recorded.
    #[serde(default)]
    pub bypass: Option<ApplyBypass>,
}

/// Why a change goes ahead without its approvals.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyBypass {
    /// At least 10 characters.
    pub reason: String,
    /// The incident it answers, such as a ticket reference.
    pub incident: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ApplyResponse {
    pub draft: DraftResponse,
    /// The configuration revision recorded for what now runs.
    pub revision: u64,
    pub revision_id: u64,
    pub content_hash: String,
    pub previous_active_hash: Option<String>,
}

impl ApplyResponse {
    fn new(draft: &DraftInfo, deployment: &ActivatedDeployment, revision: u64) -> Self {
        Self {
            draft: draft.into(),
            revision,
            revision_id: deployment.revision_id().get(),
            content_hash: deployment.content_hash().as_str().into(),
            previous_active_hash: deployment
                .previous_active_hash()
                .map(|hash| hash.as_str().into()),
        }
    }
}

macro_rules! read_route {
    ($name:ident, $path:literal, $query:expr, $body:ty, $doc:literal) => {
        #[doc = $doc]
        #[utoipa::path(get, path = $path, params(QueryHeaders), responses((status = 200, body = $body)), tag = "configuration")]
        pub(crate) async fn $name<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
        ) -> Result<Response, ApiError> {
            read(&state, &headers, $query).await
        }
    };
}

read_route!(
    list_upstreams,
    "/api/v1/upstreams",
    ModelQuery::Upstreams,
    Vec<UpstreamView>,
    "Lists upstreams with the sites that use them."
);
read_route!(
    list_listeners,
    "/api/v1/listeners",
    ModelQuery::Listeners,
    Vec<ListenerView>,
    "Lists listeners."
);
read_route!(
    list_tls_profiles,
    "/api/v1/tls-profiles",
    ModelQuery::TlsProfiles,
    Vec<TlsProfileView>,
    "Lists TLS profiles."
);
read_route!(
    list_security_policies,
    "/api/v1/security-policies",
    ModelQuery::SecurityPolicies,
    Vec<SecurityPolicyView>,
    "Lists security policies with the sites that use them."
);
read_route!(
    list_http_policies,
    "/api/v1/http-policies",
    ModelQuery::HttpPolicies,
    Vec<HttpPolicyView>,
    "Lists HTTP policies with the sites that use them."
);
read_route!(
    list_cache_policies,
    "/api/v1/cache-policies",
    ModelQuery::CachePolicies,
    Vec<CachePolicyView>,
    "Lists cache policies with the sites that use them."
);
read_route!(
    cache_settings,
    "/api/v1/cache-settings",
    ModelQuery::CacheSettings,
    CacheSettings,
    "How much the gateway's cache keeps."
);
read_route!(
    site_summary,
    "/api/v1/sites/summary",
    ModelQuery::SiteSummary,
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
    read(&state, &headers, ModelQuery::Sites { query }).await
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
        ModelChange::CreateSite {
            site: json(&headers, &body)?,
        },
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
    read(&state, &headers, ModelQuery::Site { id: path.id }).await
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
        ModelChange::ReplaceSite {
            id: path.id,
            site: json(&headers, &body)?,
        },
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
    let id = path.id;
    let command = if query.permanent {
        ModelChange::PurgeSite { id }
    } else {
        ModelChange::DeleteSite { id }
    };
    change(
        &state,
        &headers,
        command,
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

macro_rules! site_action {
    ($name:ident, $path:literal, $change:ident, $doc:literal) => {
        #[doc = $doc]
        #[utoipa::path(post, path = $path,
            params(MutationHeaders, SitePath, ("If-Match" = Option<String>, Header, description = "ETag the site must still have")),
            responses((status = 200, body = SiteView)), tag = "configuration")]
        pub(crate) async fn $name<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<SitePath>,
        ) -> Result<Response, ApiError> {
            change(&state, &headers, ModelChange::$change { id: path.id }, Precondition::Optional, StatusCode::OK).await
        }
    };
}

site_action!(
    enable_site,
    "/api/v1/sites/{id}/enable",
    EnableSite,
    "Starts serving a site."
);
site_action!(
    disable_site,
    "/api/v1/sites/{id}/disable",
    DisableSite,
    "Stops serving a site while keeping its configuration."
);
site_action!(
    favorite_site,
    "/api/v1/sites/{id}/favorite",
    FavoriteSite,
    "Pins a site."
);
site_action!(
    unfavorite_site,
    "/api/v1/sites/{id}/unfavorite",
    UnfavoriteSite,
    "Unpins a site."
);
site_action!(
    restore_site,
    "/api/v1/sites/{id}/restore",
    RestoreSite,
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
        ModelChange::CloneSite {
            id: path.id,
            name: json::<CloneSiteRequest>(&headers, &body)?.name,
        },
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
        ModelChange::BatchSites {
            batch: json(&headers, &body)?,
        },
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
    let mut response = read(&state, &headers, ModelQuery::ExportSites { ids }).await?;
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
        ModelChange::ImportSites {
            bundle: json(&headers, &body)?,
        },
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
        ModelQuery::Domains {
            site_id: query.site_id,
            q: query.q,
        },
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
    read(
        &state,
        &headers,
        ModelQuery::CheckDomains {
            hosts: json::<DomainCheckRequest>(&headers, &body)?.hosts,
        },
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
        ModelChange::AddDomains {
            site: path.id,
            domains: json(&headers, &body)?,
        },
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
        ModelChange::ReplaceDomain {
            site: path.id,
            host: host(&path.host)?,
            domain: json(&headers, &body)?,
        },
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
        ModelChange::RemoveDomain {
            site: path.id,
            host: host(&path.host)?,
        },
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

/// Lists a site's routes in evaluation order.
#[utoipa::path(get, path = "/api/v1/sites/{id}/routes", params(QueryHeaders, SitePath),
    responses((status = 200, body = Vec<RouteView>)), tag = "configuration")]
pub(crate) async fn list_routes<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
) -> Result<Response, ApiError> {
    read(&state, &headers, ModelQuery::Routes { site: path.id }).await
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
        ModelChange::CreateRoute {
            site: path.id,
            route: json(&headers, &body)?,
        },
        Precondition::Optional,
        StatusCode::CREATED,
    )
    .await
}

/// Reorders a site's routes, assigning priorities in the given order.
#[utoipa::path(put, path = "/api/v1/sites/{id}/routes/order", request_body = RouteOrderRequest,
    params(MutationHeaders, SitePath), responses((status = 200, body = Vec<RouteView>)), tag = "configuration")]
pub(crate) async fn reorder_routes<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SitePath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        ModelChange::ReorderRoutes {
            site: path.id,
            order: json::<RouteOrderRequest>(&headers, &body)?.order,
        },
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
    read(&state, &headers, ModelQuery::Route { id: path.id }).await
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
        ModelChange::ReplaceRoute {
            id: path.id,
            route: json(&headers, &body)?,
        },
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
        ModelChange::DeleteRoute { id: path.id },
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
        ModelChange::CreateUpstream {
            upstream: json(&headers, &body)?,
        },
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
    read(&state, &headers, ModelQuery::Upstream { id: path.id }).await
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
        ModelChange::ReplaceUpstream {
            id: path.id,
            upstream: json(&headers, &body)?,
        },
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
        ModelChange::DeleteUpstream { id: path.id },
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
        ModelChange::AddNode {
            upstream: path.id,
            node: json(&headers, &body)?,
        },
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
        ModelChange::ReplaceNode {
            upstream: path.id,
            id: path.node,
            node: json(&headers, &body)?,
        },
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
        ModelChange::DeleteNode {
            upstream: path.id,
            id: path.node,
        },
        Precondition::Required,
        StatusCode::OK,
    )
    .await
}

macro_rules! named_resource {
    ($get:ident, $put:ident, $delete:ident, $path:literal, $body:ty, $input:ty, $read:ident, $write:ident { $field:ident }, $remove:ident) => {
        #[utoipa::path(get, path = $path, params(QueryHeaders, NamedPath),
            responses((status = 200, body = $body, headers(("ETag" = String)))), tag = "configuration")]
        pub(crate) async fn $get<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
        ) -> Result<Response, ApiError> {
            read(&state, &headers, ModelQuery::$read { id: path.id }).await
        }

        /// Creates or replaces the resource; replacing needs `If-Match`.
        #[utoipa::path(put, path = $path, request_body = $input,
            params(MutationHeaders, NamedPath, ("If-Match" = Option<String>, Header, description = "ETag when replacing")),
            responses((status = 200, body = $body)), tag = "configuration")]
        pub(crate) async fn $put<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
            body: Bytes,
        ) -> Result<Response, ApiError> {
            let $field = json::<$input>(&headers, &body)?;
            if $field.id != path.id {
                return Err(ApiError::new(PanelError::invalid_argument(
                    "the body id must match the path",
                )));
            }
            change(&state, &headers, ModelChange::$write { $field }, Precondition::Optional, StatusCode::OK).await
        }

        #[utoipa::path(delete, path = $path,
            params(MutationHeaders, NamedPath, ("If-Match" = String, Header, description = "ETag of the resource")),
            responses((status = 200)), tag = "configuration")]
        pub(crate) async fn $delete<U: GatewayUseCases>(
            State(state): State<ApiState<U>>,
            headers: HeaderMap,
            Path(path): Path<NamedPath>,
        ) -> Result<Response, ApiError> {
            change(&state, &headers, ModelChange::$remove { id: path.id }, Precondition::Required, StatusCode::OK).await
        }
    };
}

named_resource!(
    get_listener,
    put_listener,
    delete_listener,
    "/api/v1/listeners/{id}",
    Listener,
    Listener,
    Listener,
    PutListener { listener },
    DeleteListener
);
named_resource!(
    get_tls_profile,
    put_tls_profile,
    delete_tls_profile,
    "/api/v1/tls-profiles/{id}",
    TlsProfile,
    TlsProfileInput,
    TlsProfile,
    PutTlsProfile { profile },
    DeleteTlsProfile
);
named_resource!(
    get_security_policy,
    put_security_policy,
    delete_security_policy,
    "/api/v1/security-policies/{id}",
    SecurityPolicy,
    SecurityPolicy,
    SecurityPolicy,
    PutSecurityPolicy { policy },
    DeleteSecurityPolicy
);
named_resource!(
    get_http_policy,
    put_http_policy,
    delete_http_policy,
    "/api/v1/http-policies/{id}",
    HttpPolicy,
    HttpPolicy,
    HttpPolicy,
    PutHttpPolicy { policy },
    DeleteHttpPolicy
);
named_resource!(
    get_cache_policy,
    put_cache_policy,
    delete_cache_policy,
    "/api/v1/cache-policies/{id}",
    CachePolicy,
    CachePolicy,
    CachePolicy,
    PutCachePolicy { policy },
    DeleteCachePolicy
);

/// Sets how much the gateway's cache keeps; a new size empties it once
/// applied.
#[utoipa::path(put, path = "/api/v1/cache-settings", request_body = CacheSettings,
    params(MutationHeaders, ("If-Match" = Option<String>, Header, description = "ETag of the settings")),
    responses((status = 200, body = CacheSettings)), tag = "configuration")]
pub(crate) async fn put_cache_settings<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let settings = json::<CacheSettings>(&headers, &body)?;
    change(
        &state,
        &headers,
        ModelChange::PutCacheSettings { settings },
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// The draft's version and whether the gateway runs it.
#[utoipa::path(get, path = "/api/v1/config/draft", params(QueryHeaders),
    responses((status = 200, body = DraftResponse)), tag = "configuration")]
pub(crate) async fn draft<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = request_scope(&headers)?;
    let output = port(&state)?.read(scope, ModelQuery::Draft.into()).await?;
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
    read(&state, &headers, ModelQuery::Validate { site_ids }).await
}

/// Compiles the draft and activates it on the gateway, unless approval
/// policies cover the change and it still waits for approvals.
#[utoipa::path(post, path = "/api/v1/config/apply", request_body = ApplyRequest,
    params(MutationHeaders), responses(
        (status = 200, body = ApplyResponse),
        (status = 202, body = ApprovalRequest,
            description = "Policies ask for approvals first; nothing was applied")),
    tag = "configuration")]
pub(crate) async fn apply<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    principal: Option<Extension<Principal>>,
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
    let mut apply = panel_config_api::ApplyRequest::new(request.expected_version.unwrap_or(0));
    if let Some(note) = request.note.filter(|note| !note.trim().is_empty()) {
        apply = apply.with_note(note);
    }
    if let Some(bypass) = request.bypass {
        if principal.is_some_and(|Extension(principal)| !principal.can(Permission::ApprovalBypass))
        {
            return Err(ApiError::new(PanelError::permission_denied(
                "applying without approvals needs the approval.bypass permission",
            )));
        }
        apply = apply.bypassing(ApprovalBypass::new(bypass.reason, bypass.incident));
    }
    match port(&state)?.apply(context, apply).await? {
        ApplyOutcome::Applied {
            draft,
            deployment,
            revision,
            ..
        } => {
            let mut response =
                axum::Json(ApplyResponse::new(&draft, &deployment, revision)).into_response();
            insert_draft(response.headers_mut(), &draft);
            Ok(response)
        }
        ApplyOutcome::AwaitingApproval { draft, request, .. } => {
            let mut response = (StatusCode::ACCEPTED, axum::Json(request)).into_response();
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
