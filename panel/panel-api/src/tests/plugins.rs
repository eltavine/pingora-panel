use super::*;
use chrono::Utc;
use panel_application::RequestScope;
use panel_plugin_api::{
    PluginChange, PluginCommand, PluginLimits, PluginList, PluginOutput, PluginQuery, PluginState,
    PluginView, PluginsPort, SecretView, TrustedKeyView,
};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct FakePlugins {
    reads: Mutex<Vec<PluginQuery>>,
    changes: Mutex<Vec<(PluginCommand, Option<String>)>>,
}

fn view(name: &str) -> PluginView {
    PluginView {
        name: name.into(),
        state: PluginState::Enabled,
        active_version: Some("1.0.0".into()),
        previous_version: None,
        versions: Vec::new(),
        grants: vec!["dns01".into()],
        settings: json!({"token": "vault:dns"}),
        limits: PluginLimits::default(),
        effective_limits: None,
        health: None,
        error: None,
        updated_at: Some(Utc::now()),
        etag: "\"4\"".into(),
    }
}

fn json_output(value: &impl serde::Serialize, etag: Option<&str>) -> PluginOutput {
    PluginOutput {
        content: serde_json::to_vec(value).unwrap(),
        etag: etag.map(str::to_owned),
    }
}

fn list() -> PluginList {
    PluginList {
        protocol_versions: vec![1],
        ports: vec!["dns01".into()],
        capabilities: vec!["dns01".into(), "secret-references".into()],
        limits_enforced: true,
        discovered_at: Some(Utc::now()),
        plugins: vec![view("dns")],
    }
}

#[async_trait]
impl PluginsPort for FakePlugins {
    async fn read(&self, _scope: RequestScope, query: PluginQuery) -> Result<PluginOutput> {
        self.reads.lock().unwrap().push(query.clone());
        Ok(match query {
            PluginQuery::Plugins => json_output(&list(), None),
            PluginQuery::Plugin { name } if name == "dns" => {
                json_output(&view("dns"), Some("\"4\""))
            }
            PluginQuery::Plugin { name } => {
                return Err(PanelError::not_found(format!("there is no plugin {name}")))
            }
            PluginQuery::Keys => json_output(
                &[TrustedKeyView {
                    id: "acme".into(),
                    key_id: "0123456789ABCDEF".into(),
                    public_key: "RWQ".into(),
                    comment: String::new(),
                    created_at: Utc::now(),
                }],
                None,
            ),
            PluginQuery::Secrets => json_output(
                &[SecretView {
                    name: "dns".into(),
                    updated_at: Utc::now(),
                }],
                None,
            ),
        })
    }

    async fn change(&self, context: CommandContext, change: PluginChange) -> Result<PluginOutput> {
        assert_eq!(context.actor(), "admin");
        let output = match &change.command {
            PluginCommand::Discover => json_output(&list(), None),
            PluginCommand::PutKey { key } => json_output(
                &TrustedKeyView {
                    id: key.id.clone(),
                    key_id: "0123456789ABCDEF".into(),
                    public_key: key.public_key.clone(),
                    comment: key.comment.clone(),
                    created_at: Utc::now(),
                },
                None,
            ),
            PluginCommand::PutSecret { name, .. } => json_output(
                &SecretView {
                    name: name.clone(),
                    updated_at: Utc::now(),
                },
                None,
            ),
            PluginCommand::DeleteKey { .. } | PluginCommand::DeleteSecret { .. } => PluginOutput {
                content: Vec::new(),
                etag: None,
            },
            _ => json_output(&view("dns"), Some("\"5\"")),
        };
        self.changes
            .lock()
            .unwrap()
            .push((change.command, change.if_match));
        Ok(output)
    }
}

fn app(plugins: Option<Arc<FakePlugins>>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(match plugins {
        Some(plugins) => state.with_plugins(plugins),
        None => state,
    })
}

fn request(method: &str, uri: &str, if_match: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor", "admin")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", format!("{method}-{uri}"));
    if let Some(if_match) = if_match {
        builder = builder.header(header::IF_MATCH, if_match);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Option<String>, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let etag = response
        .headers()
        .get(header::ETAG)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, etag, body)
}

#[tokio::test]
async fn plugin_reads_map_onto_the_plugins_port() {
    let plugins = Arc::new(FakePlugins::default());
    let app = app(Some(Arc::clone(&plugins)));

    let (status, _, body) = send(&app, request("GET", "/api/v1/plugins", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["protocol_versions"], json!([1]));
    assert_eq!(body["plugins"][0]["state"], "enabled");

    let (status, etag, body) = send(&app, request("GET", "/api/v1/plugins/dns", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(etag.as_deref(), Some("\"4\""));
    assert_eq!(body["settings"]["token"], "vault:dns");

    let (status, _, body) = send(&app, request("GET", "/api/v1/plugins/ftp", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "NOT_FOUND");

    let (status, _, body) = send(&app, request("GET", "/api/v1/plugin-keys", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["id"], "acme");
    let (status, _, body) = send(&app, request("GET", "/api/v1/plugin-secrets", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([{"name": "dns", "updated_at": body[0]["updated_at"]}])
    );

    assert_eq!(
        *plugins.reads.lock().unwrap(),
        vec![
            PluginQuery::Plugins,
            PluginQuery::Plugin { name: "dns".into() },
            PluginQuery::Plugin { name: "ftp".into() },
            PluginQuery::Keys,
            PluginQuery::Secrets,
        ]
    );
}

#[tokio::test]
async fn plugin_changes_map_onto_commands_with_their_entity_tags() {
    let plugins = Arc::new(FakePlugins::default());
    let app = app(Some(Arc::clone(&plugins)));
    let expect = |method: &str, uri: &str, if_match: Option<&str>, body: Option<Value>| {
        request(method, uri, if_match, body)
    };

    for (request, status) in [
        (
            expect("POST", "/api/v1/plugins/discover", None, None),
            StatusCode::OK,
        ),
        (
            expect(
                "PUT",
                "/api/v1/plugins/dns/grants",
                Some("\"4\""),
                Some(json!({"capabilities": ["dns01"]})),
            ),
            StatusCode::OK,
        ),
        (
            expect(
                "PUT",
                "/api/v1/plugins/dns/settings",
                None,
                Some(json!({"token": "vault:dns"})),
            ),
            StatusCode::OK,
        ),
        (
            expect(
                "PUT",
                "/api/v1/plugins/dns/limits",
                None,
                Some(json!({"concurrency": 4})),
            ),
            StatusCode::OK,
        ),
        (
            expect("POST", "/api/v1/plugins/dns/enable", None, None),
            StatusCode::OK,
        ),
        (
            expect(
                "POST",
                "/api/v1/plugins/dns/enable",
                None,
                Some(json!({"version": "1.0.0"})),
            ),
            StatusCode::OK,
        ),
        (
            expect(
                "POST",
                "/api/v1/plugins/dns/upgrade",
                None,
                Some(json!({"version": "1.1.0"})),
            ),
            StatusCode::OK,
        ),
        (
            expect("POST", "/api/v1/plugins/dns/rollback", None, None),
            StatusCode::OK,
        ),
        (
            expect("POST", "/api/v1/plugins/dns/disable", None, None),
            StatusCode::OK,
        ),
        (
            expect(
                "POST",
                "/api/v1/plugin-keys",
                None,
                Some(json!({"id": "acme", "public_key": "RWQ"})),
            ),
            StatusCode::CREATED,
        ),
        (
            expect("DELETE", "/api/v1/plugin-keys/acme", None, None),
            StatusCode::NO_CONTENT,
        ),
        (
            expect(
                "PUT",
                "/api/v1/plugin-secrets/dns",
                None,
                Some(json!({"value": "s3cret"})),
            ),
            StatusCode::OK,
        ),
        (
            expect("DELETE", "/api/v1/plugin-secrets/dns", None, None),
            StatusCode::NO_CONTENT,
        ),
    ] {
        let uri = request.uri().to_string();
        let (answered, etag, _) = send(&app, request).await;
        assert_eq!(answered, status, "{uri}");
        if uri.starts_with("/api/v1/plugins/dns") {
            assert_eq!(etag.as_deref(), Some("\"5\""), "{uri}");
        }
    }

    let changes = plugins.changes.lock().unwrap();
    let operations: Vec<&str> = changes
        .iter()
        .map(|(command, _)| command.operation())
        .collect();
    assert_eq!(
        operations,
        [
            "plugins.discover",
            "plugins.grant",
            "plugins.configure",
            "plugins.limit",
            "plugins.enable",
            "plugins.enable",
            "plugins.upgrade",
            "plugins.rollback",
            "plugins.disable",
            "plugins.keys.put",
            "plugins.keys.delete",
            "plugins.secrets.put",
            "plugins.secrets.delete",
        ]
    );
    assert_eq!(changes[1].1.as_deref(), Some("\"4\""));
    assert_eq!(changes[2].1, None);
    assert_eq!(
        changes[3].0,
        PluginCommand::Limit {
            name: "dns".into(),
            limits: PluginLimits {
                concurrency: 4,
                ..PluginLimits::default()
            },
        }
    );
    assert_eq!(
        changes[4].0,
        PluginCommand::Enable {
            name: "dns".into(),
            version: None
        }
    );
    assert_eq!(
        changes[5].0,
        PluginCommand::Enable {
            name: "dns".into(),
            version: Some("1.0.0".into())
        }
    );
    match &changes[11].0 {
        PluginCommand::PutSecret { name, value } => {
            assert_eq!(name, "dns");
            assert_eq!(value.expose(), "s3cret");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn plugin_requests_are_checked_before_they_reach_the_port() {
    let plugins = Arc::new(FakePlugins::default());
    let app = app(Some(Arc::clone(&plugins)));
    for request in [
        request(
            "PUT",
            "/api/v1/plugins/dns/grants",
            None,
            Some(json!({"capabilities": ["dns01"], "extra": true})),
        ),
        request(
            "PUT",
            "/api/v1/plugins/dns/limits",
            None,
            Some(json!({"memory": 1})),
        ),
        request(
            "POST",
            "/api/v1/plugins/dns/enable",
            None,
            Some(json!({"versions": "1"})),
        ),
        request("POST", "/api/v1/plugins/dns/upgrade", None, Some(json!({}))),
        request(
            "PUT",
            "/api/v1/plugin-secrets/dns",
            None,
            Some(json!({"secret": "s3cret"})),
        ),
    ] {
        let uri = request.uri().to_string();
        let (status, _, body) = send(&app, request).await;
        assert!(status.is_client_error(), "{uri}: {status}");
        assert_eq!(body["code"], "INVALID_ARGUMENT", "{uri}: {body}");
    }
    assert!(plugins.changes.lock().unwrap().is_empty());

    let (status, _, body) = send(
        &self::app(None),
        request("GET", "/api/v1/plugins", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "UNAVAILABLE");
}
