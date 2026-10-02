use super::*;
use std::sync::Mutex;

/// Records the compare-and-swap hash each activation receives.
struct CasUseCases {
    active: Option<ContentHash>,
    expected: Mutex<Vec<Option<ContentHash>>>,
}

impl CasUseCases {
    fn new(active: Option<&[u8]>) -> Arc<Self> {
        Arc::new(Self {
            active: active.map(ContentHash::from_bytes),
            expected: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl GatewayUseCases for CasUseCases {
    async fn validate(&self, _: ConfigDocument) -> Result<ValidationReport> {
        Ok(ValidationReport::valid())
    }
    async fn prepare(&self, _: CommandContext, _: ConfigDocument) -> Result<PreparedDeployment> {
        unreachable!()
    }
    async fn activate(
        &self,
        _: CommandContext,
        _: String,
        expected: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.expected.lock().unwrap().push(expected);
        Ok(ActivatedDeployment::new(
            RevisionId::new(2),
            ContentHash::from_bytes(b"next"),
            self.active.clone(),
        ))
    }
    async fn status(&self) -> Result<GatewayStatus> {
        Ok(GatewayStatus::new(
            true,
            None,
            self.active.as_ref().map(|_| RevisionId::new(1)),
            self.active.clone(),
            0,
            "fake",
            "v1",
        ))
    }
}

fn activation(if_match: Option<&str>, body: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/gateway/activate")
        .header("content-type", "application/json")
        .header("x-actor", "operator")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", "activate-1");
    if let Some(value) = if_match {
        request = request.header("if-match", value);
    }
    request.body(Body::from(body.to_owned())).unwrap()
}

fn quoted(hash: &[u8]) -> String {
    format!("\"{}\"", ContentHash::from_bytes(hash).as_str())
}

#[tokio::test]
async fn if_match_names_the_active_configuration_an_activation_replaces() {
    let use_cases = CasUseCases::new(Some(b"active"));
    let router = router(ApiState::new(Arc::clone(&use_cases)));
    let token = r#"{"prepare_token":"token"}"#;
    for value in [
        quoted(b"active"),
        "*".into(),
        format!("\"other\", {}", quoted(b"active")),
    ] {
        let response = router
            .clone()
            .oneshot(activation(Some(&value), token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{value}");
        assert_eq!(response.headers()[header::ETAG], quoted(b"next").as_str());
    }
    assert!(use_cases
        .expected
        .lock()
        .unwrap()
        .iter()
        .all(|expected| expected.as_ref() == Some(&ContentHash::from_bytes(b"active"))));

    let weak = format!("W/{}", quoted(b"active"));
    for value in [quoted(b"stale"), weak] {
        let response = router
            .clone()
            .oneshot(activation(Some(&value), token))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_FAILED,
            "{value}"
        );
    }
    assert_eq!(use_cases.expected.lock().unwrap().len(), 3);

    let ambiguous = activation(
        Some(&quoted(b"active")),
        &format!(
            r#"{{"prepare_token":"token","expected_active_hash":"{}"}}"#,
            ContentHash::from_bytes(b"active").as_str()
        ),
    );
    assert_eq!(
        router.clone().oneshot(ambiguous).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let malformed = activation(Some("active"), token);
    assert_eq!(
        router.oneshot(malformed).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn any_active_configuration_is_required_by_a_wildcard() {
    let router = router(ApiState::new(CasUseCases::new(None)));
    let response = router
        .oneshot(activation(Some("*"), r#"{"prepare_token":"token"}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
}

async fn get(
    router: &axum::Router,
    uri: &str,
    if_none_match: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(value) = if_none_match {
        request = request.header("if-none-match", value);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn status_and_receipts_revalidate_with_their_entity_tags() {
    let router = app();
    let first = get(&router, "/api/v1/gateway/status", None).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-cache");
    let tag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert!(tag.starts_with('"') && tag.ends_with('"'));

    for value in [
        tag.clone(),
        format!("W/{tag}"),
        format!("\"other\", {tag}"),
        "*".into(),
    ] {
        let revalidated = get(&router, "/api/v1/gateway/status", Some(&value)).await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED, "{value}");
        assert_eq!(revalidated.headers()[header::ETAG], tag.as_str());
        assert!(axum::body::to_bytes(revalidated.into_body(), 64)
            .await
            .unwrap()
            .is_empty());
    }
    let changed = get(&router, "/api/v1/gateway/status", Some("\"other\"")).await;
    assert_eq!(changed.status(), StatusCode::OK);

    let receipts = receipt_app(IdempotencyLookup::Completed(completed_receipt()));
    let receipt = get(&receipts, "/api/v1/gateway/receipts/key", None).await;
    let tag = receipt.headers()[header::ETAG].to_str().unwrap().to_owned();
    let revalidated = get(&receipts, "/api/v1/gateway/receipts/key", Some(&tag)).await;
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
}
