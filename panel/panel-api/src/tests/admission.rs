use super::*;
use panel_health::{
    CheckOutcome, ComponentType, HealthCheck, HealthRegistry, HealthReport, HealthWatch, Impact,
    ServiceIdentity,
};
use serde_json::Value;
use std::time::Duration;

struct Dependency(bool);

#[async_trait]
impl HealthCheck for Dependency {
    fn component(&self) -> &str {
        "postgresql"
    }
    fn component_type(&self) -> ComponentType {
        ComponentType::Datastore
    }
    async fn check(&self) -> CheckOutcome {
        if self.0 {
            CheckOutcome::pass()
        } else {
            CheckOutcome::fail("unreachable")
        }
    }
}

async fn health(impact: Impact, healthy: bool) -> HealthWatch {
    let report = HealthRegistry::new(ServiceIdentity::new("panel-api", "test"))
        .register(Arc::new(Dependency(healthy)), impact)
        .evaluate()
        .await;
    HealthWatch::fixed(report)
}

fn guarded(health: HealthWatch) -> axum::Router {
    router_with_config(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_health(health),
        ApiConfig::default().with_unavailable_retry_after(Duration::from_secs(7)),
    )
}

fn status_request() -> Request<Body> {
    Request::builder()
        .uri("/api/v1/gateway/status")
        .body(Body::empty())
        .unwrap()
}

fn prepare_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/prepare")
        .header("content-type", "application/json")
        .header("x-request-id", "degraded-write")
        .header("x-actor", "operator")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", "prepare-degraded")
        .body(Body::from(
            serde_json::json!({
                "schema_version": panel_ir::IR_SCHEMA_VERSION,
                "snapshot": RuntimeSnapshot::empty(RevisionId::new(1)),
            })
            .to_string(),
        ))
        .unwrap()
}

async fn unavailable(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "7");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["code"], "UNAVAILABLE");
    assert_eq!(body["retryable"], true);
    body
}

#[tokio::test]
async fn degraded_services_serve_reads_and_refuse_writes() {
    let router = guarded(health(Impact::Degrading, false).await);
    let read = router.clone().oneshot(status_request()).await.unwrap();
    assert_eq!(read.status(), StatusCode::OK);

    let body = unavailable(router.oneshot(prepare_request()).await.unwrap()).await;
    assert_eq!(body["request_id"], "degraded-write");
}

#[tokio::test]
async fn unavailable_services_refuse_reads_too() {
    let router = guarded(health(Impact::Required, false).await);
    unavailable(router.clone().oneshot(status_request()).await.unwrap()).await;
    unavailable(router.oneshot(prepare_request()).await.unwrap()).await;

    let starting = guarded(HealthWatch::fixed(HealthReport::starting(
        &ServiceIdentity::new("panel-api", "test"),
    )));
    unavailable(starting.oneshot(status_request()).await.unwrap()).await;
}

#[tokio::test]
async fn healthy_services_admit_everything() {
    let router = guarded(health(Impact::Degrading, true).await);
    assert_eq!(
        router
            .clone()
            .oneshot(status_request())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router.oneshot(prepare_request()).await.unwrap().status(),
        StatusCode::OK
    );
}
