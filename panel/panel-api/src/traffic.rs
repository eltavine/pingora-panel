//! What the gateway served, from its metrics (ADR 0022): request counts and
//! rates, status classes, latency, traffic, connections, upstreams and the
//! busiest routes. Counts are estimates Prometheus extrapolates from its
//! samples, so they may be fractional.

use crate::{
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    Latency, RouteTraffic, StatusClasses, TrafficPoint, TrafficPort, TrafficQuery, TrafficSummary,
    UpstreamTraffic,
};
use panel_domain::{RouteId, SiteId};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use utoipa::{IntoParams, ToSchema};

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn TrafficPort>, ApiError> {
    state.traffic.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "the gateway's traffic is not available here",
        ))
    })
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn seconds(duration: Option<Duration>) -> Option<f64> {
    duration.map(|duration| duration.as_secs_f64())
}

/// Latency quantiles in seconds; absent when no request was measured.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LatencyQuantiles {
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

impl From<Latency> for LatencyQuantiles {
    fn from(value: Latency) -> Self {
        Self {
            p50: seconds(value.p50),
            p90: seconds(value.p90),
            p95: seconds(value.p95),
            p99: seconds(value.p99),
        }
    }
}

/// Requests by the class of their status code.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct StatusCounts {
    /// 1xx.
    pub informational: f64,
    /// 2xx.
    pub success: f64,
    /// 3xx.
    pub redirection: f64,
    /// 4xx.
    pub client_error: f64,
    /// 5xx.
    pub server_error: f64,
}

impl From<StatusClasses> for StatusCounts {
    fn from(value: StatusClasses) -> Self {
        Self {
            informational: value.informational,
            success: value.success,
            redirection: value.redirection,
            client_error: value.client_error,
            server_error: value.server_error,
        }
    }
}

/// Requests the gateway sent to one upstream.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct UpstreamTrafficItem {
    pub upstream: String,
    pub requests: f64,
    /// The share of attempts that failed, from 0 to 1.
    pub error_ratio: f64,
    pub latency: LatencyQuantiles,
}

impl From<UpstreamTraffic> for UpstreamTrafficItem {
    fn from(value: UpstreamTraffic) -> Self {
        Self {
            upstream: value.upstream,
            requests: value.requests,
            error_ratio: value.error_ratio,
            latency: value.latency.into(),
        }
    }
}

/// Requests one route served.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RouteTrafficItem {
    pub site: String,
    pub route: String,
    pub requests: f64,
}

impl From<RouteTraffic> for RouteTrafficItem {
    fn from(value: RouteTraffic) -> Self {
        Self {
            site: value.site,
            route: value.route,
            requests: value.requests,
        }
    }
}

/// What the gateway served over a window.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TrafficSummaryResponse {
    /// When the figures were read, RFC 3339.
    pub observed_at: Option<String>,
    /// The window the figures cover, in seconds.
    pub window_seconds: u64,
    pub requests: f64,
    pub requests_per_second: f64,
    pub statuses: StatusCounts,
    pub latency: LatencyQuantiles,
    /// Request body bytes received.
    pub bytes_received: f64,
    /// Response body bytes sent.
    pub bytes_sent: f64,
    /// Client connections open now.
    pub open_connections: f64,
    pub tls_handshakes: f64,
    /// Busiest first.
    pub upstreams: Vec<UpstreamTrafficItem>,
    /// Busiest first, at most 20.
    pub routes: Vec<RouteTrafficItem>,
    /// The revision of the gateway's active configuration.
    pub revision: Option<u64>,
    /// When that configuration was activated, RFC 3339.
    pub activated_at: Option<String>,
}

impl From<TrafficSummary> for TrafficSummaryResponse {
    fn from(value: TrafficSummary) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            window_seconds: value.window.as_secs(),
            requests: value.requests,
            requests_per_second: value.requests_per_second,
            statuses: value.statuses.into(),
            latency: value.latency.into(),
            bytes_received: value.bytes_received,
            bytes_sent: value.bytes_sent,
            open_connections: value.open_connections,
            tls_handshakes: value.tls_handshakes,
            upstreams: value.upstreams.into_iter().map(Into::into).collect(),
            routes: value.routes.into_iter().map(Into::into).collect(),
            revision: value.revision,
            activated_at: value.activated_at.map(rfc3339),
        }
    }
}

/// The traffic at one moment, averaged over one step.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TrafficPointItem {
    /// RFC 3339.
    pub at: String,
    pub requests_per_second: f64,
    /// Requests answered with a 5xx status per second.
    pub server_errors_per_second: f64,
    /// Seconds; absent when no request was measured.
    pub p95: Option<f64>,
}

impl From<TrafficPoint> for TrafficPointItem {
    fn from(value: TrafficPoint) -> Self {
        Self {
            at: rfc3339(value.at),
            requests_per_second: value.requests_per_second,
            server_errors_per_second: value.server_errors_per_second,
            p95: seconds(value.p95),
        }
    }
}

/// The traffic over a window, oldest point first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TrafficSeriesResponse {
    pub points: Vec<TrafficPointItem>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct SummaryParams {
    /// Only the requests of this site.
    site: Option<String>,
    /// Only the requests of this route; needs `site`.
    route: Option<String>,
    /// How far back to look, in seconds: an hour by default, at least a
    /// minute and at most 31 days.
    window: Option<u64>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct SeriesParams {
    /// Only the requests of this site.
    site: Option<String>,
    /// Only the requests of this route; needs `site`.
    route: Option<String>,
    /// How far back to look, in seconds: an hour by default, at least a
    /// minute and at most 31 days.
    window: Option<u64>,
    /// Seconds between points; the service keeps a window to at most 720
    /// points.
    step: Option<u64>,
}

fn identifier<T, E>(
    name: &str,
    value: Option<String>,
    parse: impl FnOnce(String) -> Result<T, E>,
) -> Result<Option<T>, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .map(parse)
        .transpose()
        .map_err(|_| {
            ApiError::new(PanelError::invalid_argument(format!(
                "{name} is not an identifier"
            )))
        })
}

fn query(
    site: Option<String>,
    route: Option<String>,
    window: Option<u64>,
) -> Result<TrafficQuery, ApiError> {
    Ok(TrafficQuery {
        site: identifier("site", site, SiteId::new)?,
        route: identifier("route", route, RouteId::new)?,
        window: window.map(Duration::from_secs),
    })
}

/// What the gateway served over a window, for every site, one site or one
/// route.
#[utoipa::path(get, path = "/api/v1/traffic", params(QueryHeaders, SummaryParams),
    responses((status = 200, body = TrafficSummaryResponse)), tag = "traffic")]
pub(crate) async fn traffic_summary<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(params): Query<SummaryParams>,
) -> Result<Json<TrafficSummaryResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    let query = query(params.site, params.route, params.window)?;
    Ok(Json(port(&state)?.summary(scope, query).await?.into()))
}

/// The request rate, server errors and 95th percentile latency over a
/// window, for charts.
#[utoipa::path(get, path = "/api/v1/traffic/series", params(QueryHeaders, SeriesParams),
    responses((status = 200, body = TrafficSeriesResponse)), tag = "traffic")]
pub(crate) async fn traffic_series<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(params): Query<SeriesParams>,
) -> Result<Json<TrafficSeriesResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    let query = query(params.site, params.route, params.window)?;
    let points = port(&state)?
        .series(scope, query, params.step.map(Duration::from_secs))
        .await?;
    Ok(Json(TrafficSeriesResponse {
        points: points.into_iter().map(Into::into).collect(),
    }))
}
