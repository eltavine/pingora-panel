use super::{runtime::FakeRuntime, FakeGateway, IdentityCompiler};
use crate::{router, AccessSettings, ApiState, SystemInfo};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use chrono::{DateTime, Utc};
use panel_application::{
    ActivatedDeployment, ContentHash, GatewayPort, GatewayService, GatewayStatus,
    PreparedDeployment,
};
use panel_domain::RevisionId;
use panel_errors::{Result, ValidationReport};
use panel_identity::{memory::MemoryIdentityStore, Identity, IdentitySettings, SecretHash};
use panel_ir::RuntimeSnapshot;
use panel_platform::{
    ProtocolRange, ServiceDescriptor, ServiceDirectory, ServiceListing, ServiceName,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const DEPLOYMENT: &str = r#"{"changed_at":"2026-10-08T12:00:00Z","action":"upgrade",
    "engine":"podman","project":"pingora-panel","previous":"0.8.0","images":[
    {"service":"control","image":"ghcr.io/example/pingora-panel:0.9.0",
     "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"}]}"#;

struct Directory;

#[async_trait]
impl ServiceDirectory for Directory {
    async fn list(&self) -> Result<ServiceListing> {
        let started = DateTime::parse_from_rfc3339("2026-10-08T11:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let instance =
            ServiceDescriptor::new(ServiceName::new("config-service")?, "0.3.0", started)
                .with_schema_version("20")
                .with_protocol(ProtocolRange::up_to("pingora.panel.config.v1", 2)?);
        Ok(ServiceListing::new(started, vec![instance]))
    }
}

/// A gateway with a snapshot prepared and a message that names a token.
struct PreparedGateway;

#[async_trait]
impl GatewayPort for PreparedGateway {
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        FakeGateway.validate(snapshot).await
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment> {
        FakeGateway.prepare(snapshot).await
    }

    async fn activate(
        &self,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        FakeGateway
            .activate(prepare_token, expected_active_hash)
            .await
    }

    async fn status(&self) -> Result<GatewayStatus> {
        Ok(GatewayStatus::new(
            true,
            Some("prepared by Bearer abc.def-ghi with ppat_0123abcd".into()),
            Some(RevisionId::new(1)),
            Some(ContentHash::from_bytes(b"active")),
            1,
            "fake",
            panel_ir::IR_SCHEMA_VERSION,
        ))
    }
}

fn state(gateway: Arc<dyn GatewayPort>) -> ApiState<GatewayService> {
    ApiState::new(Arc::new(GatewayService::new(
        gateway,
        Arc::new(IdentityCompiler),
    )))
    .with_directory(Arc::new(Directory))
    .with_runtime(Arc::new(FakeRuntime::default()))
    .with_system(SystemInfo::new("0.9.0", "0123abc").with_deployment(DEPLOYMENT))
}

async fn get(
    app: &Router,
    uri: &str,
    bearer: Option<&str>,
) -> (StatusCode, header::HeaderMap, Value) {
    let mut request = Request::builder().uri(uri);
    if let Some(secret) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {secret}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn versions_name_the_release_the_modules_the_gateway_and_the_deployment() {
    let app = router(state(Arc::new(FakeGateway)));
    let (status, _, body) = get(&app, "/api/v1/system/versions", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["release"], "0.9.0");
    assert_eq!(body["commit"], "0123abc");
    assert_eq!(body["api"], "v1");
    assert_eq!(body["language"], panel_config_dsl::LANGUAGE_VERSION);
    assert_eq!(body["ir_schema"], panel_ir::IR_SCHEMA_VERSION);
    assert_eq!(body["modules"][0]["service"], "config-service");
    assert_eq!(body["modules"][0]["schema_version"], "20");
    assert_eq!(body["gateway"]["engine"], "0.9.0");
    assert!(body.get("agent").is_none(), "{body}");
    assert_eq!(body["deployment"]["engine"], "podman");
    assert_eq!(body["deployment"]["images"][0]["service"], "control");
    assert_eq!(body["problems"], json!([]));

    let unreadable = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_system(SystemInfo::new("0.9.0", "0123abc").with_deployment("{")),
    );
    let (_, _, body) = get(&unreadable, "/api/v1/system/versions", None).await;
    assert!(body.get("deployment").is_none());
    assert!(body["problems"][0]
        .as_str()
        .unwrap()
        .starts_with("the deployment record cannot be read"));
}

#[tokio::test]
async fn a_prepared_snapshot_holds_an_upgrade_back() {
    let ready = router(state(Arc::new(FakeGateway)));
    let (status, _, body) = get(&ready, "/api/v1/system/preflight", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ready"], true, "{body}");
    let states: Vec<(&str, &str)> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["name"].as_str().unwrap(),
                check["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [("modules", "warn"), ("gateway", "pass"), ("backup", "warn")]
    );

    let prepared = router(state(Arc::new(PreparedGateway)));
    let (_, _, body) = get(&prepared, "/api/v1/system/preflight", None).await;
    assert_eq!(body["ready"], false);
    assert_eq!(body["checks"][1]["state"], "fail");
    assert!(body["checks"][1]["detail"]
        .as_str()
        .unwrap()
        .contains("activate or abort them first"));
}

#[tokio::test]
async fn diagnostics_are_an_attachment_without_secrets() {
    let app = router(state(Arc::new(PreparedGateway)));
    let (status, headers, body) = get(&app, "/api/v1/system/diagnostics", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment; filename=\"pingora-panel-diagnostics-"));
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(body["versions"]["release"], "0.9.0");
    assert_eq!(body["readiness"]["ready"], false);
    assert_eq!(body["gateway"]["prepared_count"], 1);
    assert_eq!(
        body["gateway"]["message"],
        "prepared by Bearer [redacted] with [redacted]"
    );
    assert_eq!(body["data_plane"]["engine_version"], "0.9.0");
    assert_eq!(body["withheld"], json!([]));
    let text = body.to_string();
    assert!(
        !text.contains("abc.def-ghi") && !text.contains("ppat_0123abcd"),
        "{text}"
    );
}

#[tokio::test]
async fn the_bundle_needs_platform_diagnose_and_withholds_what_the_caller_cannot_read() {
    let identity = Identity::new(
        Arc::new(MemoryIdentityStore::default()),
        IdentitySettings {
            bootstrap: Some(SecretHash::of("the-bootstrap-token")),
            ..IdentitySettings::default()
        },
    );
    let app =
        router(state(Arc::new(FakeGateway)).with_identity(identity, AccessSettings::default()));
    let call = |method: &str, uri: &str, bearer: Option<&str>, body: Value| {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(secret) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {secret}"));
        }
        let app = app.clone();
        let request = request.body(Body::from(body.to_string())).unwrap();
        async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let password = "glacier violin tapestry orbit";
    let (status, _) = call(
        "POST",
        "/api/v1/setup",
        None,
        json!({"token": "the-bootstrap-token", "username": "root", "password": password}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, session) = call(
        "POST",
        "/api/v1/session",
        None,
        json!({"username": "root", "password": password, "transport": "bearer"}),
    )
    .await;
    let root = session["secret"].as_str().unwrap().to_owned();
    let (_, viewer) = call(
        "POST",
        "/api/v1/account/tokens",
        Some(&root),
        json!({"name": "viewer", "permissions": ["platform.read"], "expires_in_days": 1}),
    )
    .await;
    let viewer = viewer["secret"].as_str().unwrap().to_owned();
    let (_, diagnose) = call(
        "POST",
        "/api/v1/account/tokens",
        Some(&root),
        json!({"name": "support", "permissions": ["platform.diagnose"], "expires_in_days": 1}),
    )
    .await;
    let diagnose = diagnose["secret"].as_str().unwrap().to_owned();

    assert_eq!(
        get(&app, "/api/v1/system/versions", Some(&viewer)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, "/api/v1/system/preflight", Some(&viewer)).await.0,
        StatusCode::OK
    );
    let (status, _, refused) = get(&app, "/api/v1/system/diagnostics", Some(&viewer)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(refused["detail"]
        .as_str()
        .unwrap()
        .contains("platform.diagnose"));

    let (status, _, everything) = get(&app, "/api/v1/system/diagnostics", Some(&root)).await;
    assert_eq!(status, StatusCode::OK, "{everything}");
    assert_eq!(everything["withheld"], json!([]));
    assert_eq!(everything["gateway"]["ready"], true);

    let (status, _, partial) = get(&app, "/api/v1/system/diagnostics", Some(&diagnose)).await;
    assert_eq!(status, StatusCode::OK, "{partial}");
    assert!(partial.get("gateway").is_none() && partial.get("data_plane").is_none());
    assert_eq!(
        partial["withheld"],
        json!([
            "gateway: gateway.read",
            "host: host.read",
            "configuration: config.read",
            "backups: backups.read",
            "alerts: alerts.read",
            "plugins: plugins.read",
            "audit: audit.read",
        ])
    );
}
