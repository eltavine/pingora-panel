//! The operational HTTP listener: `/metrics` for Prometheus (ADR 0022).

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use panel_metrics::{Metrics, ScrapeToken, CONTENT_TYPE, PATH};
use std::sync::Arc;

struct Scrape {
    metrics: Arc<Metrics>,
    token: ScrapeToken,
}

/// Serves `metrics` at `/metrics` to scrapes that present `token`.
pub fn ops_router(metrics: Arc<Metrics>, token: ScrapeToken) -> Router {
    Router::new()
        .route(PATH, get(scrape))
        .with_state(Arc::new(Scrape { metrics, token }))
}

async fn scrape(State(scrape): State<Arc<Scrape>>, headers: HeaderMap) -> Response {
    if !scrape.token.admits(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    }
    (
        [
            (header::CONTENT_TYPE, CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        scrape.metrics.encode(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(router: Router, authorization: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::get(PATH);
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

    #[tokio::test]
    async fn scrapes_present_the_configured_token() {
        let metrics = Arc::new(Metrics::new());
        let router = ops_router(metrics, ScrapeToken::bearer("s3cret"));
        assert_eq!(get(router.clone(), None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            get(router.clone(), Some("Bearer wrong")).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) = get(router, Some("Bearer s3cret")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.ends_with("# EOF\n"), "{body}");
    }
}
