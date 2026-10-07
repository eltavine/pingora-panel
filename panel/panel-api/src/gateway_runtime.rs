//! Operations on the running gateway: its data plane, worker count,
//! shutdown, upstream health, drained nodes, file checks and proxy cache.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    CachePurge, CacheStats, DataPlaneState, EndpointHealth, FileChecks, GatewayRuntimePort,
    GatewayUseCases, PrivateKeyCheck, SiteCacheStats, StaticRootCheck, UpstreamHealth,
    UpstreamHealthReport,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::SystemTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn GatewayRuntimePort>, ApiError> {
    state.runtime.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "gateway operations are not available here",
        ))
    })
}

fn rfc3339(time: Option<SystemTime>) -> Option<String> {
    time.map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataPlaneListenerResponse {
    pub id: String,
    pub address: String,
    pub tls: bool,
    pub http1: bool,
    pub http2: bool,
}

/// The data plane as it runs now.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataPlaneResponse {
    /// Increases with every reload; zero before the first generation.
    pub generation: u64,
    pub worker_count: u32,
    pub listeners: Vec<DataPlaneListenerResponse>,
    pub generation_started_at: Option<String>,
    /// Why the configured listeners are not all served.
    pub error: Option<String>,
    pub gateway_version: String,
    /// Version of the proxy engine.
    pub engine_version: String,
    pub adapter_version: String,
    pub started_at: Option<String>,
    pub uptime_seconds: u64,
    pub observed_at: Option<String>,
    pub active_revision_id: Option<u64>,
    pub active_hash: Option<String>,
}

impl From<DataPlaneState> for DataPlaneResponse {
    fn from(state: DataPlaneState) -> Self {
        Self {
            generation: state.generation,
            worker_count: state.worker_count,
            listeners: state
                .listeners
                .into_iter()
                .map(|listener| DataPlaneListenerResponse {
                    id: listener.id,
                    address: listener.address,
                    tls: listener.tls,
                    http1: listener.http1,
                    http2: listener.http2,
                })
                .collect(),
            generation_started_at: rfc3339(state.generation_started_at),
            error: state.error,
            gateway_version: state.gateway_version,
            engine_version: state.engine_version,
            adapter_version: state.adapter_version,
            started_at: rfc3339(state.started_at),
            uptime_seconds: state.uptime_seconds,
            observed_at: rfc3339(state.observed_at),
            active_revision_id: state.active_revision_id,
            active_hash: state.active_hash.map(|hash| hash.as_str().to_owned()),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EndpointHealthResponse {
    /// The upstream node this endpoint serves.
    pub node_id: String,
    pub address: String,
    pub weight: u32,
    pub enabled: bool,
    pub backup: bool,
    /// Active health; nodes without checks stay healthy.
    pub healthy: bool,
    pub drained: bool,
    /// Set while repeated failures keep the node out of rotation.
    pub ejected_until: Option<String>,
    pub in_flight: u32,
    pub requests: u64,
    pub failures: u64,
    /// Smoothed time to the upstream response header.
    pub latency_us: Option<u64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct UpstreamHealthResponse {
    pub upstream_id: String,
    /// Whether active health checks run for this upstream.
    pub checked: bool,
    pub nodes: Vec<EndpointHealthResponse>,
}

impl From<UpstreamHealth> for UpstreamHealthResponse {
    fn from(health: UpstreamHealth) -> Self {
        Self {
            upstream_id: health.upstream_id,
            checked: health.checked,
            nodes: health
                .endpoints
                .into_iter()
                .map(|endpoint: EndpointHealth| EndpointHealthResponse {
                    node_id: endpoint.endpoint_id,
                    address: endpoint.address,
                    weight: endpoint.weight,
                    enabled: endpoint.enabled,
                    backup: endpoint.backup,
                    healthy: endpoint.healthy,
                    drained: endpoint.drained,
                    ejected_until: rfc3339(endpoint.ejected_until),
                    in_flight: endpoint.in_flight,
                    requests: endpoint.requests,
                    failures: endpoint.failures,
                    latency_us: endpoint.latency_us,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct UpstreamHealthReportResponse {
    pub upstreams: Vec<UpstreamHealthResponse>,
    pub observed_at: Option<String>,
    pub active_revision_id: Option<u64>,
    pub active_hash: Option<String>,
}

impl From<UpstreamHealthReport> for UpstreamHealthReportResponse {
    fn from(report: UpstreamHealthReport) -> Self {
        Self {
            upstreams: report.upstreams.into_iter().map(Into::into).collect(),
            observed_at: rfc3339(report.observed_at),
            active_revision_id: report.active_revision_id,
            active_hash: report.active_hash.map(|hash| hash.as_str().to_owned()),
        }
    }
}

/// A TLS private key in the gateway's secret directory.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PrivateKeyCheckResponse {
    pub file: String,
    /// TLS profiles serving it.
    pub tls_profile_ids: Vec<String>,
    /// Octal Unix permission bits, such as `0600`, where the platform has
    /// them.
    pub mode: Option<String>,
    /// Whether only the gateway's user may read or write the file.
    pub owner_only: bool,
    /// Why the file could not be inspected.
    pub error: Option<String>,
}

impl From<PrivateKeyCheck> for PrivateKeyCheckResponse {
    fn from(check: PrivateKeyCheck) -> Self {
        Self {
            file: check.file,
            tls_profile_ids: check.tls_profile_ids,
            mode: check.mode.map(|mode| format!("{mode:04o}")),
            owner_only: check.owner_only,
            error: check.error,
        }
    }
}

/// A link below a static root that leads out of it; it is not served.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EscapingLinkResponse {
    /// The link, relative to the root.
    pub path: String,
    pub target: String,
}

/// A static site's root directory.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct StaticRootCheckResponse {
    /// The static content the root serves.
    pub id: String,
    pub root: String,
    /// Whether the root resolves to a directory inside the static content
    /// root.
    pub inside: bool,
    pub escaping_links: Vec<EscapingLinkResponse>,
    pub entries_checked: u32,
    /// Whether the root holds more entries than one check looks at.
    pub truncated: bool,
    pub error: Option<String>,
}

impl From<StaticRootCheck> for StaticRootCheckResponse {
    fn from(check: StaticRootCheck) -> Self {
        Self {
            id: check.id,
            root: check.root,
            inside: check.inside,
            escaping_links: check
                .escaping_links
                .into_iter()
                .map(|link| EscapingLinkResponse {
                    path: link.path,
                    target: link.target,
                })
                .collect(),
            entries_checked: check.entries_checked,
            truncated: check.truncated,
            error: check.error,
        }
    }
}

/// The most sites or URLs one purge names.
const MOST_PURGED: usize = 100;

/// What the proxy cache holds and did since the gateway started (ADR 0043).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CacheStatsResponse {
    pub observed_at: Option<String>,
    /// When the counts started: the gateway's start.
    pub since: Option<String>,
    pub bytes: u64,
    pub entries: u64,
    pub max_bytes: u64,
    pub sites: Vec<SiteCacheStatsResponse>,
}

/// Requests of a site by what the cache did for them.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteCacheStatsResponse {
    pub site_id: String,
    pub hits: u64,
    /// Served stale while revalidated or when the upstream failed.
    pub stale: u64,
    /// Served stale while another request revalidated.
    pub updating: u64,
    pub misses: u64,
    /// Found stale and fetched again.
    pub expired: u64,
    /// Found stale and confirmed by the upstream with 304.
    pub revalidated: u64,
    pub bypasses: u64,
    /// Fetched without being stored, as their responses said.
    pub uncacheable: u64,
    /// Of the requests the cache looked up, the share it answered, when it
    /// looked any up.
    pub hit_ratio: Option<f64>,
}

impl From<SiteCacheStats> for SiteCacheStatsResponse {
    fn from(site: SiteCacheStats) -> Self {
        let answered = site.hits + site.stale + site.updating + site.revalidated;
        let looked_up = answered + site.misses + site.expired + site.uncacheable;
        Self {
            hit_ratio: (looked_up > 0).then(|| answered as f64 / looked_up as f64),
            site_id: site.site_id,
            hits: site.hits,
            stale: site.stale,
            updating: site.updating,
            misses: site.misses,
            expired: site.expired,
            revalidated: site.revalidated,
            bypasses: site.bypasses,
            uncacheable: site.uncacheable,
        }
    }
}

impl From<CacheStats> for CacheStatsResponse {
    fn from(stats: CacheStats) -> Self {
        Self {
            observed_at: rfc3339(stats.observed_at),
            since: rfc3339(stats.since),
            bytes: stats.bytes,
            entries: stats.entries,
            max_bytes: stats.max_bytes,
            sites: stats.sites.into_iter().map(Into::into).collect(),
        }
    }
}

/// What to purge from the proxy cache: everything, sites, or URLs; exactly
/// one of them.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CachePurgeRequest {
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub site_ids: Vec<Uuid>,
    /// Absolute URLs of the active configuration's sites; every variant
    /// stored for each goes.
    #[serde(default)]
    pub urls: Vec<String>,
}

impl CachePurgeRequest {
    fn purge(self) -> Result<CachePurge, PanelError> {
        let named = [self.all, !self.site_ids.is_empty(), !self.urls.is_empty()]
            .iter()
            .filter(|named| **named)
            .count();
        if named != 1 {
            return Err(PanelError::invalid_argument(
                "a purge names exactly one of all, site_ids and urls",
            ));
        }
        if self.site_ids.len() > MOST_PURGED || self.urls.len() > MOST_PURGED {
            return Err(PanelError::invalid_argument(format!(
                "a purge names at most {MOST_PURGED} sites or URLs"
            )));
        }
        Ok(if self.all {
            CachePurge::All
        } else if self.urls.is_empty() {
            CachePurge::Sites(self.site_ids.iter().map(Uuid::to_string).collect())
        } else {
            CachePurge::Urls(self.urls)
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CachePurgeResponse {
    /// How many keys the URLs came to; zero for sites and everything.
    pub keys: u64,
}

/// What the proxy cache holds and did for each site since the gateway
/// started.
#[utoipa::path(get, path = "/api/v1/gateway/cache", params(QueryHeaders),
    responses((status = 200, body = CacheStatsResponse)), tag = "gateway")]
pub(crate) async fn cache_stats<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<CacheStatsResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(port(&state)?.cache_stats(scope).await?.into()))
}

/// Purges the proxy cache: everything at once, a site's responses, or URLs;
/// what is purged is fetched again on its next request.
#[utoipa::path(post, path = "/api/v1/gateway/cache/purge", request_body = CachePurgeRequest,
    params(MutationHeaders), responses((status = 200, body = CachePurgeResponse)), tag = "gateway")]
pub(crate) async fn purge_cache<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Json(request): Json<CachePurgeRequest>,
) -> Result<Json<CachePurgeResponse>, ApiError> {
    let context = command_context(&headers)?;
    let purge = request.purge()?;
    let purged = port(&state)?.purge_cache(context, purge).await?;
    Ok(Json(CachePurgeResponse { keys: purged.keys }))
}

/// What the gateway finds in the files the active configuration serves
/// from.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FileChecksResponse {
    pub checked_at: Option<String>,
    pub active_revision_id: Option<u64>,
    pub private_keys: Vec<PrivateKeyCheckResponse>,
    pub static_roots: Vec<StaticRootCheckResponse>,
}

impl From<FileChecks> for FileChecksResponse {
    fn from(checks: FileChecks) -> Self {
        Self {
            checked_at: rfc3339(checks.checked_at),
            active_revision_id: checks.active_revision_id,
            private_keys: checks.private_keys.into_iter().map(Into::into).collect(),
            static_roots: checks.static_roots.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerCountRequest {
    pub worker_count: u32,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ShutdownResponse {
    pub accepted: bool,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct NodePath {
    /// Upstream identifier.
    id: Uuid,
    node: Uuid,
}

/// The data plane's generation, workers, listeners and versions.
#[utoipa::path(get, path = "/api/v1/gateway/data-plane", params(QueryHeaders),
    responses((status = 200, body = DataPlaneResponse)), tag = "gateway")]
pub(crate) async fn data_plane<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<DataPlaneResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(port(&state)?.data_plane(scope).await?.into()))
}

/// Starts a new listener generation and drains the previous one.
#[utoipa::path(post, path = "/api/v1/gateway/reload", params(MutationHeaders),
    responses((status = 200, body = DataPlaneResponse)), tag = "gateway")]
pub(crate) async fn reload<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<DataPlaneResponse>, ApiError> {
    let context = command_context(&headers)?;
    Ok(Json(port(&state)?.reload(context).await?.into()))
}

/// Changes the number of data plane workers through a reload; it persists
/// across restarts.
#[utoipa::path(put, path = "/api/v1/gateway/workers", request_body = WorkerCountRequest,
    params(MutationHeaders), responses((status = 200, body = DataPlaneResponse)), tag = "gateway")]
pub(crate) async fn workers<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Json(request): Json<WorkerCountRequest>,
) -> Result<Json<DataPlaneResponse>, ApiError> {
    let context = command_context(&headers)?;
    Ok(Json(
        port(&state)?
            .set_worker_count(context, request.worker_count)
            .await?
            .into(),
    ))
}

/// Drains in-flight requests and stops the gateway process.
#[utoipa::path(post, path = "/api/v1/gateway/shutdown", params(MutationHeaders),
    responses((status = 202, body = ShutdownResponse)), tag = "gateway")]
pub(crate) async fn shutdown<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    port(&state)?.shutdown(context).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ShutdownResponse { accepted: true }),
    )
        .into_response())
}

/// Live health, load and latency of every upstream node.
#[utoipa::path(get, path = "/api/v1/upstreams/health", params(QueryHeaders),
    responses((status = 200, body = UpstreamHealthReportResponse)), tag = "gateway")]
pub(crate) async fn upstream_health<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<UpstreamHealthReportResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(port(&state)?.upstream_health(scope).await?.into()))
}

/// Whether TLS private keys may be read only by their owner and static
/// roots stay inside the static content root.
#[utoipa::path(get, path = "/api/v1/gateway/file-checks", params(QueryHeaders),
    responses((status = 200, body = FileChecksResponse)), tag = "gateway")]
pub(crate) async fn file_checks<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<FileChecksResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(port(&state)?.file_checks(scope).await?.into()))
}

/// Takes a node out of rotation until it is restored; this survives restarts.
#[utoipa::path(put, path = "/api/v1/upstreams/{id}/nodes/{node}/drain", params(MutationHeaders, NodePath),
    responses((status = 200, body = UpstreamHealthResponse)), tag = "gateway")]
pub(crate) async fn drain<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<NodePath>,
) -> Result<Json<UpstreamHealthResponse>, ApiError> {
    set_drained(state, headers, path, true).await
}

/// Returns a drained node to rotation.
#[utoipa::path(delete, path = "/api/v1/upstreams/{id}/nodes/{node}/drain", params(MutationHeaders, NodePath),
    responses((status = 200, body = UpstreamHealthResponse)), tag = "gateway")]
pub(crate) async fn restore<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<NodePath>,
) -> Result<Json<UpstreamHealthResponse>, ApiError> {
    set_drained(state, headers, path, false).await
}

async fn set_drained<U>(
    state: ApiState<U>,
    headers: HeaderMap,
    path: NodePath,
    drained: bool,
) -> Result<Json<UpstreamHealthResponse>, ApiError> {
    let context = command_context(&headers)?;
    Ok(Json(
        port(&state)?
            .set_endpoint_drained(context, path.id.to_string(), path.node.to_string(), drained)
            .await?
            .into(),
    ))
}
