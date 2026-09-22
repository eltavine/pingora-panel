#![forbid(unsafe_code)]

#[path = "support/management.rs"]
mod management;

use management::{json, problem, ManagementServer};
use panel_domain::{ContentHash, RevisionId};
use panel_engine::{FakeGatewayEngine, GatewayEngine};
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use serde_json::{json as value, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn document(revision: u64) -> Value {
    value!({"schema_version": IR_SCHEMA_VERSION, "snapshot": RuntimeSnapshot::empty(RevisionId::new(revision))})
}

async fn prepare(server: &ManagementServer, revision: u64) -> Value {
    json(
        server
            .mutation(
                "/api/v1/gateway/prepare",
                &document(revision),
                &format!("prepare-{revision}"),
            )
            .send()
            .await
            .unwrap(),
        200,
    )
    .await
}

#[tokio::test]
async fn configuration_publication_replay_cas_and_receipts_work_over_http() {
    let server = ManagementServer::start(
        Arc::new(FakeGatewayEngine::with_default_capabilities()),
        8192,
    )
    .await;
    let validated = json(
        server
            .post("/api/v1/gateway/validate", &document(1))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(validated["valid"], true);
    let prepared = prepare(&server, 1).await;
    assert_eq!(prepared["revision_id"], 1);
    assert_eq!(
        prepared["content_hash"],
        document(1)["snapshot"]["content_hash"]
    );
    let request = value!({"prepare_token": prepared["prepare_token"]});
    let active = json(
        server
            .mutation("/api/v1/gateway/activate", &request, "publish-1")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(active["revision_id"], prepared["revision_id"]);
    assert_eq!(active["content_hash"], prepared["content_hash"]);
    let replay = json(
        server
            .mutation("/api/v1/gateway/activate", &request, "publish-1")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(replay, active);
    let status = json(
        server.get("/api/v1/gateway/status").send().await.unwrap(),
        200,
    )
    .await;
    assert_eq!(status["active_revision_id"], active["revision_id"]);
    assert_eq!(status["active_hash"], active["content_hash"]);
    assert_eq!(status["prepared_count"], 0);
    let receipt = json(
        server
            .get("/api/v1/gateway/receipts/publish-1")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(receipt["outcome"]["status"], "succeeded");
    assert_eq!(receipt["outcome"]["revision_id"], active["revision_id"]);
    assert_eq!(receipt["outcome"]["content_hash"], active["content_hash"]);

    let conflicting_request = value!({"prepare_token": "different-token"});
    problem(
        server
            .mutation(
                "/api/v1/gateway/activate",
                &conflicting_request,
                "publish-1",
            )
            .send()
            .await
            .unwrap(),
        409,
        "CONFLICT",
    )
    .await;

    let second = prepare(&server, 2).await;
    let stale = value!({"prepare_token": second["prepare_token"], "expected_active_hash": ContentHash::from_bytes(b"stale").as_str()});
    problem(
        server
            .mutation("/api/v1/gateway/activate", &stale, "publish-2")
            .send()
            .await
            .unwrap(),
        409,
        "CONFLICT",
    )
    .await;
    let unchanged = json(
        server.get("/api/v1/gateway/status").send().await.unwrap(),
        200,
    )
    .await;
    assert_eq!(unchanged["active_hash"], active["content_hash"]);
    // A confirmed CAS rejection releases the key, allowing a corrected attempt.
    let corrected = value!({"prepare_token": second["prepare_token"], "expected_active_hash": active["content_hash"]});
    let updated = json(
        server
            .mutation("/api/v1/gateway/activate", &corrected, "publish-2")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(updated["revision_id"], 2);
    assert_eq!(updated["previous_active_hash"], active["content_hash"]);
    assert_eq!(updated["content_hash"], second["content_hash"]);
    server.stop().await;
}

#[tokio::test]
async fn schema_deadline_and_size_rejections_do_not_change_gateway_state() {
    let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
    let server = ManagementServer::start(engine.clone(), 2048).await;
    let mut wrong_schema = document(1);
    wrong_schema["schema_version"] = value!("unsupported");
    problem(
        server
            .post("/api/v1/gateway/validate", &wrong_schema)
            .send()
            .await
            .unwrap(),
        422,
        "UNSUPPORTED_CAPABILITY",
    )
    .await;
    let mut mismatched = document(1);
    mismatched["snapshot"]["schema_version"] = value!("different");
    problem(
        server
            .post("/api/v1/gateway/validate", &mismatched)
            .send()
            .await
            .unwrap(),
        400,
        "INVALID_ARGUMENT",
    )
    .await;
    problem(
        server
            .mutation_with_deadline(
                "/api/v1/gateway/prepare",
                &document(1),
                "expired",
                "2000-01-01T00:00:00Z",
            )
            .send()
            .await
            .unwrap(),
        408,
        "DEADLINE_EXCEEDED",
    )
    .await;
    problem(
        server
            .post(
                "/api/v1/gateway/validate",
                &value!({"schema_version": IR_SCHEMA_VERSION, "snapshot": "x".repeat(4096)}),
            )
            .send()
            .await
            .unwrap(),
        413,
        "INVALID_ARGUMENT",
    )
    .await;
    problem(
        server
            .get("/api/v1/gateway/receipts/missing")
            .send()
            .await
            .unwrap(),
        404,
        "NOT_FOUND",
    )
    .await;
    let status = engine.status().await.unwrap();
    assert!(status.active_hash.is_none());
    assert_eq!(status.prepared_count, 0);
    server.stop().await;
}

/// Simulates losing commit confirmation after the engine has applied the change.
struct UncertainEngine {
    inner: FakeGatewayEngine,
    activations: AtomicUsize,
}

#[async_trait::async_trait]
impl GatewayEngine for UncertainEngine {
    async fn capabilities(&self) -> panel_errors::Result<panel_engine::EngineCapabilities> {
        self.inner.capabilities().await
    }
    async fn validate(
        &self,
        snapshot: RuntimeSnapshot,
    ) -> panel_errors::Result<panel_errors::ValidationReport> {
        self.inner.validate(snapshot).await
    }
    async fn prepare(
        &self,
        request: panel_engine::PrepareRequest,
    ) -> panel_errors::Result<panel_engine::PrepareReceipt> {
        self.inner.prepare(request).await
    }
    async fn activate(
        &self,
        request: panel_engine::ActivateRequest,
    ) -> panel_errors::Result<panel_engine::ActivationReceipt> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        self.inner.activate(request).await?;
        Err(panel_errors::PanelError::commit_outcome_unknown(
            "activation confirmation unavailable",
        ))
    }
    async fn abort(
        &self,
        token: panel_engine::PrepareToken,
    ) -> panel_errors::Result<panel_engine::AbortReceipt> {
        self.inner.abort(token).await
    }
    async fn status(&self) -> panel_errors::Result<panel_engine::GatewayStatus> {
        self.inner.status().await
    }
}

#[tokio::test]
async fn uncertain_publication_stays_queryable_without_redispatch() {
    let engine = Arc::new(UncertainEngine {
        inner: FakeGatewayEngine::with_default_capabilities(),
        activations: AtomicUsize::new(0),
    });
    let server = ManagementServer::start(engine.clone(), 8192).await;
    let prepared = prepare(&server, 1).await;
    let request = value!({"prepare_token": prepared["prepare_token"]});
    problem(
        server
            .mutation("/api/v1/gateway/activate", &request, "uncertain")
            .send()
            .await
            .unwrap(),
        500,
        "COMMIT_OUTCOME_UNKNOWN",
    )
    .await;
    let pending = server
        .get("/api/v1/gateway/receipts/uncertain")
        .send()
        .await
        .unwrap();
    assert_eq!(pending.headers()["retry-after"], "1");
    assert_eq!(json(pending, 202).await["status"], "in_progress");
    problem(
        server
            .mutation("/api/v1/gateway/activate", &request, "uncertain")
            .send()
            .await
            .unwrap(),
        429,
        "RESOURCE_EXHAUSTED",
    )
    .await;
    problem(
        server
            .mutation(
                "/api/v1/gateway/activate",
                &value!({"prepare_token":"another-token"}),
                "uncertain",
            )
            .send()
            .await
            .unwrap(),
        409,
        "CONFLICT",
    )
    .await;
    assert_eq!(engine.activations.load(Ordering::SeqCst), 1);
    assert_eq!(
        engine.status().await.unwrap().active_revision_id,
        Some(RevisionId::new(1))
    );
    server.stop().await;
}
