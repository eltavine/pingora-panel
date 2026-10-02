use super::*;
use axum::http::HeaderValue;
use serde_json::Value;

async fn problem(
    response: axum::response::Response,
    status: StatusCode,
    expected: Option<&str>,
) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(panel_application::RequestId::new(&id).is_ok());
    if let Some(expected) = expected {
        assert_eq!(id, expected);
    }
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["request_id"], id);
    assert_eq!(body["status"], status.as_u16());
    body
}

#[tokio::test]
async fn early_rejections_and_read_errors_keep_provided_or_generated_identity() {
    for id in [None, Some("operator-request-7")] {
        for (method, path, body, status) in [
            (
                "POST",
                "/api/v1/gateway/validate",
                "{",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/gateway/prepare",
                "{}",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/v1/gateway/activate",
                "{}",
                StatusCode::BAD_REQUEST,
            ),
            (
                "GET",
                "/api/v1/gateway/receipts/missing",
                "",
                StatusCode::NOT_FOUND,
            ),
            (
                "GET",
                "/api/v1/gateway/receipts/%FF",
                "",
                StatusCode::BAD_REQUEST,
            ),
            (
                "GET",
                "/api/v1/gateway/status",
                "",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json");
            if let Some(id) = id {
                request = request.header("x-request-id", id);
            }
            let response = receipt_app(IdempotencyLookup::Missing)
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            problem(response, status, id).await;
        }
    }
}

#[tokio::test]
async fn invalid_or_ambiguous_identifiers_are_replaced_before_error_rendering() {
    for values in [
        vec![HeaderValue::from_static("")],
        vec![HeaderValue::from_static("bad\tvalue")],
        vec![HeaderValue::from_str(&"x".repeat(257)).unwrap()],
        vec![HeaderValue::from_bytes(b"\xff").unwrap()],
        vec![
            HeaderValue::from_static("first"),
            HeaderValue::from_static("second"),
        ],
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/gateway/prepare")
            .body(Body::from("{}"))
            .unwrap();
        for value in &values {
            request.headers_mut().append("x-request-id", value.clone());
        }
        let response = app().oneshot(request).await.unwrap();
        for value in values {
            assert_ne!(response.headers()["x-request-id"], value);
        }
        problem(response, StatusCode::BAD_REQUEST, None).await;
    }
}

struct FailedUseCases;

#[async_trait]
impl GatewayUseCases for FailedUseCases {
    async fn validate(&self, _: ConfigDocument) -> Result<ValidationReport> {
        Err(PanelError::internal(
            "private path /var/lib/panel/secret-key",
        ))
    }
    async fn prepare(&self, _: CommandContext, _: ConfigDocument) -> Result<PreparedDeployment> {
        Err(PanelError::storage_unavailable(
            "database password is hidden",
        ))
    }
    async fn activate(
        &self,
        _: CommandContext,
        _: String,
        _: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        Err(PanelError::conflict("active hash changed"))
    }
    async fn status(&self) -> Result<GatewayStatus> {
        Err(PanelError::internal(
            "private path /var/lib/panel/secret-key",
        ))
    }
}

#[tokio::test]
async fn application_failures_and_body_limits_keep_request_identity() {
    let failed = router(ApiState::new(Arc::new(FailedUseCases)));
    for (method, path, body, expected) in [
        (
            "POST",
            "/api/v1/gateway/validate",
            r#"{"schema_version":"v1","snapshot":{}}"#,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            "POST",
            "/api/v1/gateway/prepare",
            r#"{"schema_version":"v1","snapshot":{}}"#,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            "POST",
            "/api/v1/gateway/activate",
            r#"{"prepare_token":"token"}"#,
            StatusCode::CONFLICT,
        ),
        (
            "GET",
            "/api/v1/gateway/status",
            "",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-request-id", "application-error")
            .header("x-actor", "operator")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", "activation")
            .body(Body::from(body))
            .unwrap();
        let body = problem(
            failed.clone().oneshot(request).await.unwrap(),
            expected,
            Some("application-error"),
        )
        .await;
        if expected.is_server_error() {
            assert!(!body["detail"].as_str().unwrap().contains("/var/lib"));
            assert!(!body["detail"].as_str().unwrap().contains("password"));
            assert!(body.get("field_errors").is_none());
        } else {
            assert_eq!(body["detail"], "active hash changed");
        }
    }
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .body(Body::from("x".repeat(65)))
        .unwrap();
    problem(
        app_with_config(ApiConfig::new(32).unwrap())
            .oneshot(request)
            .await
            .unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
        None,
    )
    .await;
}

#[tokio::test]
async fn concurrent_requests_never_share_error_identity() {
    let router = app();
    let mut tasks = Vec::new();
    for index in 0..32 {
        let router = router.clone();
        tasks.push(tokio::spawn(async move {
            let id = format!("request-{index}");
            let request = Request::builder()
                .method("POST")
                .uri("/api/v1/gateway/prepare")
                .header("x-request-id", &id)
                .body(Body::empty())
                .unwrap();
            problem(
                router.oneshot(request).await.unwrap(),
                StatusCode::BAD_REQUEST,
                Some(&id),
            )
            .await;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}

/// Records the identity each use case receives.
#[derive(Default)]
struct RecordingUseCases {
    scopes: std::sync::Mutex<Vec<panel_application::RequestScope>>,
    commands: std::sync::Mutex<Vec<CommandContext>>,
}

#[async_trait]
impl GatewayUseCases for RecordingUseCases {
    async fn validate(&self, _: ConfigDocument) -> Result<ValidationReport> {
        unreachable!("queries must use their scoped variant")
    }
    async fn prepare(
        &self,
        context: CommandContext,
        _: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        self.commands.lock().unwrap().push(context);
        PreparedDeployment::new(RevisionId::new(1), ContentHash::from_bytes(b"x"), "token")
    }
    async fn activate(
        &self,
        _: CommandContext,
        _: String,
        _: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        unreachable!()
    }
    async fn status(&self) -> Result<GatewayStatus> {
        unreachable!("queries must use their scoped variant")
    }
    async fn validate_with_scope(
        &self,
        scope: panel_application::RequestScope,
        _: ConfigDocument,
    ) -> Result<ValidationReport> {
        self.scopes.lock().unwrap().push(scope);
        Ok(ValidationReport::valid())
    }
    async fn status_with_scope(
        &self,
        scope: panel_application::RequestScope,
    ) -> Result<GatewayStatus> {
        self.scopes.lock().unwrap().push(scope);
        Ok(GatewayStatus::new(
            true,
            None,
            None,
            None,
            0,
            "pingora-v1",
            "v1",
        ))
    }
}

const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[tokio::test]
async fn queries_and_commands_carry_request_correlation_and_trace_identity() {
    let use_cases = Arc::new(RecordingUseCases::default());
    let router = router(ApiState::new(Arc::clone(&use_cases)));

    let mut status = Request::builder()
        .uri("/api/v1/gateway/status")
        .header("x-request-id", "status-1")
        .header("x-correlation-id", "flow-9")
        .header("traceparent", TRACEPARENT)
        .body(Body::empty())
        .unwrap();
    status
        .headers_mut()
        .append("tracestate", HeaderValue::from_static("rojo=1"));
    status
        .headers_mut()
        .append("tracestate", HeaderValue::from_static("congo=2"));
    assert_eq!(router.clone().oneshot(status).await.unwrap().status(), 200);

    let validate = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/validate")
        .header("content-type", "application/json")
        .header("x-request-id", "validate-1")
        .body(Body::from(r#"{"schema_version":"v1","snapshot":{}}"#))
        .unwrap();
    assert_eq!(
        router.clone().oneshot(validate).await.unwrap().status(),
        200
    );

    let prepare = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/prepare")
        .header("content-type", "application/json")
        .header("x-request-id", "prepare-1")
        .header("x-actor", "operator")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", "prepare-key")
        .header("traceparent", TRACEPARENT)
        .body(Body::from(r#"{"schema_version":"v1","snapshot":{}}"#))
        .unwrap();
    assert_eq!(router.oneshot(prepare).await.unwrap().status(), 200);

    let scopes = use_cases.scopes.lock().unwrap().clone();
    assert_eq!(scopes[0].request_id().as_str(), "status-1");
    assert_eq!(scopes[0].correlation_id().as_str(), "flow-9");
    let trace = scopes[0].trace_context().unwrap();
    assert_eq!(trace.traceparent(), TRACEPARENT);
    assert_eq!(trace.tracestate(), Some("rojo=1,congo=2"));
    assert_eq!(scopes[1].request_id().as_str(), "validate-1");
    assert_eq!(scopes[1].correlation_id().as_str(), "validate-1");
    assert!(scopes[1].trace_context().is_none());

    let commands = use_cases.commands.lock().unwrap().clone();
    assert_eq!(commands[0].request_id().as_str(), "prepare-1");
    assert_eq!(
        commands[0].trace_context().unwrap().traceparent(),
        TRACEPARENT
    );
    assert_eq!(commands[0].scope().correlation_id().as_str(), "prepare-1");
}

#[tokio::test]
async fn invalid_or_repeated_traceparents_are_ignored_without_rejecting_the_request() {
    let use_cases = Arc::new(RecordingUseCases::default());
    let router = router(ApiState::new(Arc::clone(&use_cases)));
    let invalid = Request::builder()
        .uri("/api/v1/gateway/status")
        .header(
            "traceparent",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(router.clone().oneshot(invalid).await.unwrap().status(), 200);

    let mut repeated = Request::builder()
        .uri("/api/v1/gateway/status")
        .body(Body::empty())
        .unwrap();
    for _ in 0..2 {
        repeated
            .headers_mut()
            .append("traceparent", HeaderValue::from_static(TRACEPARENT));
    }
    assert_eq!(router.oneshot(repeated).await.unwrap().status(), 200);

    let scopes = use_cases.scopes.lock().unwrap().clone();
    assert!(scopes.iter().all(|scope| scope.trace_context().is_none()));
}
