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
        Err(PanelError::internal("validation unavailable"))
    }
    async fn prepare(&self, _: CommandContext, _: ConfigDocument) -> Result<PreparedDeployment> {
        Err(PanelError::storage_unavailable("storage unavailable"))
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
        Err(PanelError::internal("status unavailable"))
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
        problem(
            failed.clone().oneshot(request).await.unwrap(),
            expected,
            Some("application-error"),
        )
        .await;
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
