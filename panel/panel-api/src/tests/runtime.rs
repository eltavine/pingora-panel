use super::*;
use panel_application::{
    DataPlaneListener, DataPlaneState, EndpointHealth, GatewayRuntimePort, RequestScope,
    UpstreamHealth, UpstreamHealthReport,
};
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct FakeRuntime {
    pub(super) calls: Mutex<Vec<String>>,
}

fn state(workers: u32) -> DataPlaneState {
    let mut state = DataPlaneState::default();
    state.generation = 3;
    state.worker_count = workers;
    let mut listener = DataPlaneListener::new("https", "0.0.0.0:443");
    listener.tls = true;
    listener.http2 = true;
    state.listeners = vec![listener];
    state.engine_version = "0.9.0".into();
    state.active_revision_id = Some(7);
    state
}

fn upstream(drained: bool) -> UpstreamHealth {
    let mut endpoint = EndpointHealth::default();
    endpoint.endpoint_id = "01a0ff37-e4de-72a3-852f-e94a9797bc86".into();
    endpoint.healthy = true;
    endpoint.drained = drained;
    endpoint.latency_us = Some(1500);
    let mut health = UpstreamHealth::default();
    health.upstream_id = "01a0ff37-e4e0-7663-bd8e-0b23821a8315".into();
    health.endpoints = vec![endpoint];
    health
}

#[async_trait]
impl GatewayRuntimePort for FakeRuntime {
    async fn data_plane(&self, _scope: RequestScope) -> Result<DataPlaneState> {
        Ok(state(4))
    }

    async fn reload(&self, context: CommandContext) -> Result<DataPlaneState> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("reload {}", context.actor()));
        Ok(state(4))
    }

    async fn set_worker_count(
        &self,
        _context: CommandContext,
        workers: u32,
    ) -> Result<DataPlaneState> {
        if workers == 0 {
            return Err(PanelError::invalid_argument("workers must be positive"));
        }
        Ok(state(workers))
    }

    async fn shutdown(&self, _context: CommandContext) -> Result<()> {
        self.calls.lock().unwrap().push("shutdown".into());
        Ok(())
    }

    async fn upstream_health(&self, _scope: RequestScope) -> Result<UpstreamHealthReport> {
        let mut report = UpstreamHealthReport::default();
        report.upstreams = vec![upstream(false)];
        report.active_revision_id = Some(7);
        Ok(report)
    }

    async fn set_endpoint_drained(
        &self,
        _context: CommandContext,
        upstream_id: String,
        endpoint: String,
        drained: bool,
    ) -> Result<UpstreamHealth> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("drain {upstream_id} {endpoint} {drained}"));
        Ok(upstream(drained))
    }
}

fn runtime_app(runtime: Arc<FakeRuntime>) -> axum::Router {
    router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_runtime(runtime),
    )
}

fn mutation(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor", "operator")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", format!("{method}-{uri}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn data_plane_operations_map_onto_the_runtime_port() {
    let runtime = Arc::new(FakeRuntime::default());
    let app = runtime_app(Arc::clone(&runtime));
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/gateway/data-plane")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["listeners"][0]["tls"], true);
    assert_eq!(body["engine_version"], "0.9.0");
    assert_eq!(body["active_revision_id"], 7);

    let response = app
        .clone()
        .oneshot(mutation(
            "PUT",
            "/api/v1/gateway/workers",
            Body::from(r#"{"worker_count":8}"#),
        ))
        .await
        .unwrap();
    assert_eq!(json(response).await["worker_count"], 8);
    let response = app
        .clone()
        .oneshot(mutation(
            "PUT",
            "/api/v1/gateway/workers",
            Body::from(r#"{"worker_count":0}"#),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(mutation("POST", "/api/v1/gateway/reload", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(mutation("POST", "/api/v1/gateway/shutdown", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        *runtime.calls.lock().unwrap(),
        ["reload operator", "shutdown"]
    );
}

#[tokio::test]
async fn upstream_health_and_drains_use_model_identifiers() {
    let runtime = Arc::new(FakeRuntime::default());
    let app = runtime_app(Arc::clone(&runtime));
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/upstreams/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json(response).await;
    assert_eq!(body["upstreams"][0]["nodes"][0]["latency_us"], 1500);
    let path = "/api/v1/upstreams/01a0ff37-e4e0-7663-bd8e-0b23821a8315/nodes/01a0ff37-e4de-72a3-852f-e94a9797bc86/drain";
    let response = app
        .clone()
        .oneshot(mutation("PUT", path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(json(response).await["nodes"][0]["drained"], true);
    let response = app
        .clone()
        .oneshot(mutation("DELETE", path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(json(response).await["nodes"][0]["drained"], false);
    let response = app
        .oneshot(mutation(
            "PUT",
            "/api/v1/upstreams/not-an-id/nodes/x/drain",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(runtime.calls.lock().unwrap()[0].ends_with("true"));
}

#[tokio::test]
async fn runtime_routes_are_unavailable_without_a_gateway() {
    let response = app()
        .oneshot(
            Request::get("/api/v1/gateway/data-plane")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
