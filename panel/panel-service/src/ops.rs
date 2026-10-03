use axum::http::HeaderMap;
use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use panel_health::{HealthReport, HealthWatch, HEALTH_MEDIA_TYPE};
use panel_metrics::{Metrics, ScrapeToken};
use std::sync::Arc;

pub const LIVENESS_PATH: &str = "/livez";
pub const READINESS_PATH: &str = "/readyz";

/// Liveness and readiness endpoints answering from the published health,
/// and the process's metrics.
///
/// Liveness passes whenever the process answers; readiness reports the
/// aggregated checks and fails with 503 when a required dependency fails.
/// The documents name internal dependencies, so the router belongs on an
/// operational listener rather than the public API.
pub fn ops_router(health: HealthWatch, metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route(LIVENESS_PATH, get(liveness))
        .route(READINESS_PATH, get(readiness))
        .with_state(health)
        .route(
            panel_metrics::PATH,
            get(move |headers: HeaderMap| {
                let metrics = Arc::clone(&metrics);
                async move {
                    panel_metrics::scrape(&metrics, &ScrapeToken::none(), &headers).into_response()
                }
            }),
        )
}

async fn liveness(State(health): State<HealthWatch>) -> Response {
    health_response(&health.current().liveness())
}

async fn readiness(State(health): State<HealthWatch>) -> Response {
    health_response(&health.current())
}

/// An `application/health+json` response that is never cached.
pub fn health_response(report: &HealthReport) -> Response {
    let status =
        StatusCode::from_u16(report.http_status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    let mut response = (status, Json(report)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(HEALTH_MEDIA_TYPE),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
