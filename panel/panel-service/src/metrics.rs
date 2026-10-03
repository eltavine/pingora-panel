//! Metrics of a service process (ADR 0022): its readiness and the requests
//! its HTTP API serves, by route template.

use axum::{
    extract::{MatchedPath, Request, State},
    http::header,
    middleware::{self, Next},
    response::Response,
    Router,
};
use hyper::body::Body as _;
use panel_health::HealthWatch;
use panel_metrics::{
    method, protocol_version, ErrorType, HttpServerMetrics, Metrics, RoutedRequest,
};
use prometheus_client::{
    collector::Collector,
    encoding::{DescriptorEncoder, EncodeMetric},
    metrics::{gauge::ConstGauge, TypedMetric},
};
use std::time::Instant;

/// Reports at every scrape whether the process is ready, as
/// `pingora_panel_service_ready` 1 or 0.
pub fn register_readiness(metrics: &mut Metrics, health: HealthWatch) {
    metrics
        .registry()
        .register_collector(Box::new(Readiness(health)));
}

struct Readiness(HealthWatch);

impl std::fmt::Debug for Readiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readiness").finish_non_exhaustive()
    }
}

impl Collector for Readiness {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        let ready = ConstGauge::new(i64::from(self.0.current().http_status() == 200));
        ready.encode(encoder.encode_descriptor(
            "pingora_panel_service_ready",
            "Whether the process is ready to serve",
            None,
            ConstGauge::<i64>::TYPE,
        )?)
    }
}

/// Measures every request `router` routes, labelled by its route template
/// rather than its path, so the labels stay bounded.
pub fn measured(router: Router, metrics: HttpServerMetrics<RoutedRequest>) -> Router {
    router.route_layer(middleware::from_fn_with_state(metrics, measure))
}

async fn measure(
    State(metrics): State<HttpServerMetrics<RoutedRequest>>,
    request: Request,
    next: Next,
) -> Response {
    let http_request_method = method(request.method());
    let network_protocol_version = protocol_version(request.version());
    let http_route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let received = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok()?.parse().ok())
        .unwrap_or_default();
    let active = metrics.start(http_request_method, "http");
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    metrics.finish(
        &RoutedRequest {
            http_request_method,
            url_scheme: "http",
            http_route,
            http_response_status_code: Some(status),
            network_protocol_version,
            error_type: (status >= 500).then_some(ErrorType::Status(status)),
        },
        started.elapsed(),
        received,
        response.body().size_hint().exact().unwrap_or_default(),
    );
    drop(active);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn requests_are_labelled_by_their_route_template() {
        let mut metrics = Metrics::new();
        let api = HttpServerMetrics::<RoutedRequest>::register(metrics.registry());
        let router = measured(
            Router::new().route("/api/v1/sites/{id}", get(|| async { "shop" })),
            api,
        );
        let response = router
            .oneshot(
                Request::get("/api/v1/sites/shop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(metrics.encode().contains(
            "http_server_request_duration_seconds_count{http_request_method=\"GET\",\
             url_scheme=\"http\",http_route=\"/api/v1/sites/{id}\",http_response_status_code=\"200\",\
             network_protocol_version=\"1.1\",error_type=\"\"} 1"
        ));
    }
}
