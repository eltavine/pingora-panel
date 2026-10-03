//! The operational HTTP listener: liveness, readiness and `/metrics` for
//! Prometheus (ADR 0022).

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use panel_metrics::{Metrics, ScrapeToken, PATH};
use std::sync::Arc;
use tonic_health::{
    pb::{health_check_response::ServingStatus, health_server::Health, HealthCheckRequest},
    server::HealthService,
};

/// The draft "Health Check Response Format for HTTP APIs" media type.
const HEALTH_MEDIA_TYPE: &str = "application/health+json";

struct Ops {
    metrics: Arc<Metrics>,
    token: ScrapeToken,
    health: HealthService,
}

/// Serves `/livez` (and `/healthz`) while the process answers, `/readyz`
/// while `health` reports the gateway serving, and `metrics` at `/metrics`
/// to scrapes that present `token`.
pub fn ops_router(metrics: Arc<Metrics>, token: ScrapeToken, health: HealthService) -> Router {
    Router::new()
        .route("/livez", get(live))
        .route("/healthz", get(live))
        .route("/readyz", get(ready))
        .route(PATH, get(scrape))
        .with_state(Arc::new(Ops {
            metrics,
            token,
            health,
        }))
}

fn health(pass: bool) -> Response {
    let (status, body) = if pass {
        (StatusCode::OK, r#"{"status":"pass"}"#)
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"fail"}"#)
    };
    (
        status,
        [
            (header::CONTENT_TYPE, HEALTH_MEDIA_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn live() -> Response {
    health(true)
}

/// Ready exactly when the gateway's gRPC health says it is serving, so
/// recovery, degradation and draining read the same over both.
async fn ready(State(ops): State<Arc<Ops>>) -> Response {
    let serving = ops
        .health
        .check(tonic::Request::new(HealthCheckRequest {
            service: String::new(),
        }))
        .await
        .is_ok_and(|response| response.into_inner().status == ServingStatus::Serving as i32);
    health(serving)
}

async fn scrape(State(ops): State<Arc<Ops>>, headers: HeaderMap) -> Response {
    panel_metrics::scrape(&ops.metrics, &ops.token, &headers).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tonic_health::server::HealthReporter;
    use tower::ServiceExt;

    async fn get(router: Router, path: &str, authorization: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::get(path);
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        let response = router
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn router(reporter: &HealthReporter) -> Router {
        ops_router(
            Arc::new(Metrics::new()),
            ScrapeToken::bearer("s3cret"),
            HealthService::from_health_reporter(reporter.clone()),
        )
    }

    #[tokio::test]
    async fn scrapes_present_the_configured_token() {
        let router = router(&HealthReporter::new());
        assert_eq!(
            get(router.clone(), PATH, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(router.clone(), PATH, Some("Bearer wrong")).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) = get(router, PATH, Some("Bearer s3cret")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.ends_with("# EOF\n"), "{body}");
    }

    #[tokio::test]
    async fn readiness_follows_the_grpc_health_of_the_gateway() {
        let reporter = HealthReporter::new();
        reporter
            .set_service_status("", tonic_health::ServingStatus::NotServing)
            .await;
        let router = router(&reporter);
        assert_eq!(get(router.clone(), "/livez", None).await.0, StatusCode::OK);
        assert_eq!(
            get(router.clone(), "/healthz", None).await.0,
            StatusCode::OK
        );
        let (status, body) = get(router.clone(), "/readyz", None).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"fail"}"#)
        );
        reporter
            .set_service_status("", tonic_health::ServingStatus::Serving)
            .await;
        assert_eq!(get(router, "/readyz", None).await.0, StatusCode::OK);
    }
}
