//! Operations on the running gateway: its data plane, worker count,
//! shutdown, upstream health and drained nodes.

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
    DataPlaneState, EndpointHealth, GatewayRuntimePort, GatewayUseCases, UpstreamHealth,
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
