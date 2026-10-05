use super::*;
use axum::http::HeaderMap;
use panel_application::{CommandContext, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationCommand, ConfigurationOutput,
    ConfigurationPort, ConfigurationQuery, DraftInfo, LanguageChange, LanguageQuery,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Mutex};

/// A draft's version and files.
struct Draft(Mutex<(u64, BTreeMap<String, String>)>);

fn output(version: u64, files: &BTreeMap<String, String>) -> ConfigurationOutput {
    let etag = format!("\"draft-{version}\"");
    ConfigurationOutput {
        content: serde_json::to_vec(&json!({
            "language_version": 1,
            "version": version,
            "etag": etag,
            "files": files,
            "diagnostics": [],
        }))
        .unwrap(),
        etag: Some(etag),
        draft: DraftInfo {
            version,
            ..DraftInfo::default()
        },
    }
}

#[async_trait]
impl ConfigurationPort for Draft {
    async fn read(
        &self,
        _: RequestScope,
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        let ConfigurationQuery::Language(LanguageQuery::Source) = query else {
            return Err(PanelError::unavailable("only the files are read here"));
        };
        let draft = self.0.lock().unwrap();
        Ok(output(draft.0, &draft.1))
    }

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        assert_eq!(context.actor(), "ops");
        let ConfigurationCommand::Language(LanguageChange::ReplaceSource { files }) =
            change.command
        else {
            return Err(PanelError::unavailable("only the files are replaced here"));
        };
        let mut draft = self.0.lock().unwrap();
        *draft = (draft.0 + 1, files);
        Ok(output(draft.0, &draft.1))
    }

    async fn apply(&self, _: CommandContext, _: ApplyRequest) -> Result<ApplyOutcome> {
        Err(PanelError::unavailable("nothing is applied here"))
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn import(bundle: &Value) -> Request<Body> {
    Request::put("/api/v1/config/bundle")
        .header("content-type", "application/json")
        .header("x-actor", "ops")
        .header("idempotency-key", "bundle-1")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .body(Body::from(bundle.to_string()))
        .unwrap()
}

#[tokio::test]
async fn the_configuration_travels_as_one_bundle() {
    let draft = Arc::new(Draft(Mutex::new((
        4,
        BTreeMap::from([("main.conf".to_owned(), "server shop {}\n".to_owned())]),
    ))));
    let app = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_configuration(draft.clone()),
    );

    let (status, headers, bundle) = send(
        &app,
        Request::get("/api/v1/config/bundle")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=\"configuration-v4.json\""
    );
    assert_eq!(headers["etag"], "\"draft-4\"");
    assert_eq!(
        bundle,
        json!({
            "format": "pingora-panel-configuration",
            "language_version": 1,
            "files": { "main.conf": "server shop {}\n" },
        })
    );

    let mut imported = bundle.clone();
    imported["files"]["blog.conf"] = json!("server blog {}\n");
    let (status, headers, saved) = send(&app, import(&imported)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(headers["etag"], "\"draft-5\"");
    assert_eq!(draft.0.lock().unwrap().1.len(), 2);

    let mut foreign = imported.clone();
    foreign["format"] = json!("nginx");
    let (status, _, problem) = send(&app, import(&foreign)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    let mut newer = imported;
    newer["language_version"] = json!(panel_config_dsl::LANGUAGE_VERSION + 1);
    let (status, _, problem) = send(&app, import(&newer)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "VALIDATION_FAILED");
    assert_eq!(
        draft.0.lock().unwrap().0,
        5,
        "refused bundles change nothing"
    );
}
