use crate::{router, router_with_config, ApiConfig, ApiState, SnapshotEnvelope};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use panel_application::{
    ActivatedDeployment, ConfigCompiler, ConfigDocument, ContentHash, DeploymentOutcome,
    GatewayPort, GatewayService, GatewayStatus, IdempotencyRecord, PreparedDeployment,
};
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use std::sync::Arc;
use tower::ServiceExt;

struct IdentityCompiler;

#[async_trait]
impl ConfigCompiler for IdentityCompiler {
    async fn compile(&self, document: ConfigDocument) -> Result<RuntimeSnapshot> {
        if document.media_type() != "application/json" {
            return Err(PanelError::invalid_argument("unsupported media type"));
        }
        let snapshot: RuntimeSnapshot = serde_json::from_slice(document.body())
            .map_err(|error| PanelError::invalid_argument(error.to_string()))?;
        if document.schema_version() != snapshot.schema_version {
            return Err(PanelError::invalid_argument(
                "envelope schema version does not match the document",
            ));
        }
        Ok(snapshot)
    }
}

struct FakeGateway;

#[async_trait]
impl GatewayPort for FakeGateway {
    async fn validate(&self, _snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        Ok(ValidationReport::valid())
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment> {
        PreparedDeployment::new(snapshot.revision_id, snapshot.content_hash, "prepare-1")
    }

    async fn activate(
        &self,
        _prepare_token: String,
        _expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        Ok(ActivatedDeployment::new(
            RevisionId::new(1),
            ContentHash::from_bytes(b"active"),
            None,
        ))
    }

    async fn status(&self) -> Result<GatewayStatus> {
        Ok(GatewayStatus::new(
            true,
            Some("ready".into()),
            Some(RevisionId::new(1)),
            Some(ContentHash::from_bytes(b"active")),
            0,
            "fake",
            panel_ir::IR_SCHEMA_VERSION,
        ))
    }
}

fn app() -> axum::Router {
    router(ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    ))))
}

fn app_with_config(config: ApiConfig) -> axum::Router {
    router_with_config(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        ))),
        config,
    )
}

#[tokio::test]
async fn mutation_without_idempotency_key_is_problem_details() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/prepare")
        .header("content-type", "application/json")
        .header("x-request-id", "request-1")
        .header("x-actor", "test")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .body(Body::from(
            serde_json::to_vec(&SnapshotEnvelope {
                schema_version: panel_ir::IR_SCHEMA_VERSION.into(),
                snapshot: serde_json::to_value(snapshot).unwrap(),
            })
            .unwrap(),
        ))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn mutation_without_deadline_is_problem_details() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/prepare")
        .header("content-type", "application/json")
        .header("x-request-id", "request-1")
        .header("x-actor", "test")
        .header("idempotency-key", "idem-1")
        .body(Body::from(
            serde_json::to_vec(&SnapshotEnvelope {
                schema_version: panel_ir::IR_SCHEMA_VERSION.into(),
                snapshot: serde_json::to_value(snapshot).unwrap(),
            })
            .unwrap(),
        ))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn openapi_is_exposed_from_the_same_router() {
    let request = Request::builder()
        .uri("/api/v1/openapi.json")
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn status_flows_through_the_application_port() {
    let request = Request::builder()
        .uri("/api/v1/gateway/status")
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("\"ready\":true"));
    assert!(body.contains("\"adapter_version\":\"fake\""));
}

#[tokio::test]
async fn unconfigured_receipt_repository_is_an_explicit_capability_error() {
    let request = Request::builder()
        .uri("/api/v1/gateway/receipts/missing-key")
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("receipt queries are not configured"));
}

#[test]
fn receipt_projection_is_a_stable_public_union() {
    let deployment =
        ActivatedDeployment::new(RevisionId::new(7), ContentHash::from_bytes(b"active"), None);
    let record = IdempotencyRecord::new(
        ContentHash::from_bytes(b"request"),
        DeploymentOutcome::Succeeded(deployment),
    );
    let response = crate::IdempotencyReceiptResponse::from(record);
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["outcome"]["status"], "succeeded");
    assert_eq!(value["outcome"]["revision_id"], 7);
    assert!(value["outcome"].get("prepare_token").is_none());
}

#[tokio::test]
async fn validation_flows_through_the_application_use_case() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&SnapshotEnvelope {
                schema_version: panel_ir::IR_SCHEMA_VERSION.into(),
                snapshot: serde_json::to_value(snapshot).unwrap(),
            })
            .unwrap(),
        ))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn schema_mismatch_is_rejected_before_gateway_dispatch() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&SnapshotEnvelope {
                schema_version: "pingora.panel.ir/v2".into(),
                snapshot: serde_json::to_value(snapshot).unwrap(),
            })
            .unwrap(),
        ))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn malformed_json_uses_the_public_problem_details_media_type() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .body(Body::from("{"))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!response.headers()["x-request-id"].is_empty());
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn malformed_mutation_preserves_request_id_in_problem_details() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/activate")
        .header("content-type", "application/json")
        .header("x-request-id", "request-42")
        .header("x-actor", "test")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", "idem-42")
        .body(Body::from("{"))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["x-request-id"], "request-42");
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("\"request_id\":\"request-42\""));
}

#[tokio::test]
async fn configured_body_limit_returns_problem_details_with_413() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            "{{\"snapshot\":\"{}\"}}",
            "x".repeat(64)
        )))
        .unwrap();
    let response = app_with_config(ApiConfig::new(32).unwrap())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[test]
fn zero_body_limit_is_rejected_during_composition() {
    assert!(ApiConfig::new(0).is_err());
}
