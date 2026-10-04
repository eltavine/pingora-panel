#![forbid(unsafe_code)]

//! Configuration resources through the public HTTP API, from creation to
//! applying the draft on a gateway.

mod support;

use gateway_grpc::GatewayGrpcService;
use panel_control_runtime::{ProcessSettings, RunningProcess, DATA_DIR_ENV, NATS_URL_ENV};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_health::ServiceMode;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_service::Environment;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

async fn gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let engine = FakeGatewayEngine::new(
        [
            "action.respond",
            "activation.cas",
            "listener.http",
            "listener.http2",
            "request.security",
            "route.path-prefix",
            "upstream.http",
        ]
        .map(|name| EngineCapability::new(name, "1")),
    );
    let gateway = GatewayGrpcService::new(Arc::new(engine));
    tokio::spawn(
        Server::builder()
            .add_service(health)
            .add_service(gateway.transport_policy().gateway_server(gateway))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    address
}

fn environment(values: Vec<(&'static str, String)>) -> Environment<'static> {
    let values: HashMap<&str, OsString> = values
        .into_iter()
        .map(|(key, value)| (key, OsString::from(value)))
        .collect();
    Environment::from_lookup(move |name| values.get(name).cloned())
}

fn local(settings: ProcessSettings) -> ProcessSettings {
    settings
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50))
}

/// Serving reads and writes; optional dependencies a test does not start
/// may still be reported as failing.
async fn ready(process: &RunningProcess) {
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().mode() != ServiceMode::Normal {
            assert!(health.changed().await);
        }
    })
    .await
    .expect("the process becomes ready");
}

struct Api {
    client: Client,
    base: String,
}

impl Api {
    fn get(&self, path: &str) -> RequestBuilder {
        self.client.get(format!("{}{path}", self.base))
    }

    fn mutate(&self, method: reqwest::Method, path: &str, key: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", key)
    }

    async fn json(
        &self,
        request: RequestBuilder,
        expected: StatusCode,
    ) -> (Value, reqwest::header::HeaderMap) {
        let response = request.send().await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, expected, "{body}");
        (body, headers)
    }
}

/// config-service, audit-service and the API with a signed-in
/// Administrator, `operator`.
struct Stack {
    api: Api,
    config: RunningProcess,
    audit: RunningProcess,
    server: RunningProcess,
    _data: tempfile::TempDir,
    broker: TestBroker,
    _web: tempfile::TempDir,
}

async fn stack() -> Option<Stack> {
    let broker = TestBroker::create().await?;
    let data = tempfile::tempdir().unwrap();
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let gateway = gateway().await;
    let mut config_env = environment(vec![
        (DATA_DIR_ENV, data.path().display().to_string()),
        (NATS_URL_ENV, nats.clone()),
        (config_service::GATEWAY_URL_ENV, format!("http://{gateway}")),
    ]);
    let config_settings =
        local(ProcessSettings::read(&mut config_env, config_service::default_addresses()).unwrap());
    let config = config_service::process(&mut config_env, config_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&config).await;
    let mut audit_env = environment(vec![
        (DATA_DIR_ENV, data.path().display().to_string()),
        (NATS_URL_ENV, nats.clone()),
    ]);
    let audit_settings =
        local(ProcessSettings::read(&mut audit_env, audit_service::default_addresses()).unwrap());
    let audit = audit_service::process(&mut audit_env, audit_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&audit).await;
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<!doctype html>").unwrap();
    let http = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut api_env = environment(vec![
        (DATA_DIR_ENV, data.path().display().to_string()),
        (NATS_URL_ENV, nats),
        (panel_api_server::HTTP_ADDRESS_ENV, http.to_string()),
        (
            panel_api_server::BOOTSTRAP_TOKEN_ENV,
            support::BOOTSTRAP.into(),
        ),
        (
            panel_api_server::CONFIG_URL_ENV,
            format!("http://{}", config.grpc_address().unwrap()),
        ),
        (
            panel_api_server::WEB_ROOT_ENV,
            web.path().display().to_string(),
        ),
        (
            panel_api_server::AUDIT_URL_ENV,
            format!("http://{}", audit.grpc_address().unwrap()),
        ),
    ]);
    let api_settings =
        local(ProcessSettings::read(&mut api_env, panel_api_server::default_addresses()).unwrap());
    let server = panel_api_server::process(&mut api_env, api_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&server).await;
    let base = format!("http://{http}");
    let api = Api {
        client: support::signed_in(&base).await,
        base,
    };
    Some(Stack {
        api,
        config,
        audit,
        server,
        _data: data,
        broker,
        _web: web,
    })
}

impl Stack {
    async fn stop(self) {
        self.server.stop().await;
        self.audit.stop().await;
        self.config.stop().await;
        let _ = self
            .broker
            .context
            .delete_key_value(self.broker.settings.service_bucket())
            .await;
        self.broker.drop().await;
    }
}

#[tokio::test]
async fn sites_are_edited_validated_and_applied_through_the_api() {
    let Some(stack) = stack().await else {
        return;
    };
    let api = &stack.api;
    use reqwest::Method;

    let (listener, _) = api
        .json(
            api.mutate(Method::PUT, "/api/v1/listeners/http", "listener")
                .json(&json!({"id": "http", "address": "0.0.0.0:8080"})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(listener["protocols"]["http2"], true);
    let (upstream, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/upstreams", "upstream")
                .json(&json!({
                    "name": "app",
                    "nodes": [{"host": "127.0.0.1", "port": 9000, "weight": 3}]
                })),
            StatusCode::CREATED,
        )
        .await;
    let (site, headers) = api
        .json(
            api.mutate(Method::POST, "/api/v1/sites", "site")
                .json(&json!({
                    "name": "Shop",
                    "action": {"type": "proxy", "upstream_id": upstream["id"]},
                    "domains": [{"host": "shop.example.com", "primary": true}],
                    "tags": ["prod"]
                })),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(headers["x-config-version"], "3");
    let etag = headers["etag"].to_str().unwrap().to_owned();
    assert_eq!(site["etag"], etag.as_str());
    let site_path = format!("/api/v1/sites/{}", site["id"].as_str().unwrap());

    let (list, headers) = api
        .json(
            api.get("/api/v1/sites?q=shop&tag=prod&limit=10"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["status"], "running");
    assert!(headers.get("last-modified").is_some());
    let (summary, _) = api
        .json(api.get("/api/v1/sites/summary"), StatusCode::OK)
        .await;
    assert_eq!(summary["reverse_proxy"], 1);

    let replacement = json!({
        "name": "Shop",
        "action": {"type": "respond", "status": 503, "body": "maintenance", "retry_after_seconds": 60},
        "domains": [{"host": "shop.example.com", "primary": true}]
    });
    let missing = api
        .mutate(Method::PUT, &site_path, "replace-unconditional")
        .json(&replacement)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::PRECONDITION_REQUIRED);
    let stale = api
        .mutate(Method::PUT, &site_path, "replace-stale")
        .header("if-match", "\"stale\"")
        .json(&replacement)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(stale.headers()["content-type"], "application/problem+json");
    let (replaced, _) = api
        .json(
            api.mutate(Method::PUT, &site_path, "replace")
                .header("if-match", &etag)
                .json(&replacement),
            StatusCode::OK,
        )
        .await;
    assert_eq!(replaced["kind"], "maintenance");

    let (checks, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/domains/check", api.base))
                .json(&json!({"hosts": ["Shop.Example.com", "bücher.example", "not a host"]})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(checks[0]["owner"]["site_name"], "Shop");
    assert_eq!(checks[1]["host"], "xn--bcher-kva.example");
    assert!(checks[2]["error"].is_string());

    let routes_path = format!("{site_path}/routes");
    let mut ids = Vec::new();
    for (index, prefix) in ["/a", "/b"].iter().enumerate() {
        let (route, _) = api
            .json(
                api.mutate(Method::POST, &routes_path, &format!("route-{index}"))
                    .json(&json!({
                        "priority": 100,
                        "match": {"kind": "prefix", "path": prefix},
                        "action": {"type": "respond", "status": 204}
                    })),
                StatusCode::CREATED,
            )
            .await;
        ids.push(route["id"].clone());
    }
    let (ordered, _) = api
        .json(
            api.mutate(Method::PUT, &format!("{routes_path}/order"), "reorder")
                .json(&json!({"order": [ids[1], ids[0]]})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(ordered[0]["id"], ids[1]);

    let (applied, headers) = api
        .json(
            api.mutate(Method::POST, "/api/v1/config/apply", "apply")
                .header("x-request-id", "req-audit-apply")
                .json(&json!({"expected_version": 7})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(applied["revision_id"], 7);
    assert_eq!(applied["revision"], 1);
    assert_eq!(applied["draft"]["pending"], false);
    assert_eq!(headers["x-config-applied-version"], "7");
    let conflict = api
        .mutate(Method::POST, "/api/v1/config/apply", "apply-stale")
        .json(&json!({"expected_version": 6}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let (bundle, headers) = api
        .json(api.get("/api/v1/sites/export"), StatusCode::OK)
        .await;
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .contains("sites.json"));
    let duplicate = api
        .mutate(Method::POST, "/api/v1/sites/import", "import")
        .json(&bundle)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    let problem: Value = duplicate.json().await.unwrap();
    assert!(problem.to_string().contains("already"), "{problem}");

    let (current, _) = api.json(api.get(&site_path), StatusCode::OK).await;
    assert_ne!(
        current["etag"], replaced["etag"],
        "routes are part of the site"
    );
    let (deleted, _) = api
        .json(
            api.mutate(Method::DELETE, &site_path, "delete")
                .header("if-match", current["etag"].as_str().unwrap()),
            StatusCode::OK,
        )
        .await;
    assert_eq!(deleted["status"], "deleted");
    let (restored, _) = api
        .json(
            api.mutate(Method::POST, &format!("{site_path}/restore"), "restore"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(restored["status"], "running");
    let (draft, _) = api
        .json(api.get("/api/v1/config/draft"), StatusCode::OK)
        .await;
    assert_eq!(draft["pending"], true);

    let (source, headers) = api
        .json(api.get("/api/v1/config/source"), StatusCode::OK)
        .await;
    let etag = headers["etag"].to_str().unwrap().to_owned();
    let text = source["files"]["main.conf"].as_str().unwrap().to_owned();
    assert!(text.contains("server_name shop.example.com;"), "{text}");
    let (checked, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/config/check", api.base))
                .json(&json!({"files": {"main.conf": "language_version 1;\nhttp { server s { proxy nowhere; } }\n"}})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(checked["valid"], false);
    assert_eq!(
        checked["diagnostics"][0]["source_span"],
        "main.conf:2.25-31"
    );
    let (formatted, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/config/format", api.base))
                .json(&json!({"files": {"main.conf": "language_version 1 ;"}})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(formatted["files"]["main.conf"], "language_version 1;\n");
    let (schema, _) = api
        .json(api.get("/api/v1/config/schema"), StatusCode::OK)
        .await;
    assert!(schema["directives"].as_array().unwrap().len() > 40);
    let (tree, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/config/ast", api.base))
                .json(&json!({"files": {"main.conf": "language_version 1;\n"}})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(tree["directives"][0]["span"], "main.conf:1.1-19");
    let explained_text = "language_version 1;\nhttp {\n    listener edge {\n        address 127.0.0.1:8081;\n    }\n    server s {\n        server_name s.example;\n        respond 204;\n    }\n}\n";
    let (explained, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/config/explain", api.base))
                .json(&json!({"files": {"main.conf": explained_text}, "file": "main.conf", "line": 8})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(explained["block"], "server");
    assert!(explained["settings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|setting| setting["name"] == "listen"
            && setting["value"] == "edge"
            && setting["source"] == "default"));
    api.json(
        api.client
            .post(format!("{}/api/v1/config/explain", api.base))
            .json(&json!({"files": {"main.conf": explained_text}, "file": "main.conf", "line": 1})),
        StatusCode::NOT_FOUND,
    )
    .await;
    let (imported, _) = api
        .json(
            api.client
                .post(format!("{}/api/v1/config/import/nginx", api.base))
                .json(&json!({
                    "entry": "nginx.conf",
                    "files": {"nginx.conf": "events {}\nhttp {\n    server {\n        listen 8081;\n        server_name import.example;\n        location / { proxy_pass http://127.0.0.1:9000; }\n    }\n}\n"}
                })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(imported["valid"], true, "{imported}");
    assert!(imported["files"]["main.conf"]
        .as_str()
        .unwrap()
        .contains("server_name import.example;"));
    assert_eq!(imported["report"][0]["code"], "NGINX_UNSUPPORTED");
    assert_eq!(imported["report"][0]["source_span"], "nginx.conf:1.1-9");
    let (ir, _) = api.json(api.get("/api/v1/config/ir"), StatusCode::OK).await;
    assert_eq!(ir["sites"].as_array().unwrap().len(), 1);

    let renamed = text.replace("shop.example.com", "store.example.com");
    let stale = api
        .mutate(Method::PUT, "/api/v1/config/source", "source-stale")
        .header("if-match", "\"draft-1\"")
        .json(&json!({"files": {"main.conf": renamed}}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    let (saved, _) = api
        .json(
            api.mutate(Method::PUT, "/api/v1/config/source", "source")
                .header("if-match", &etag)
                .json(&json!({"files": {"main.conf": renamed}})),
            StatusCode::OK,
        )
        .await;
    assert!(saved["files"]["main.conf"]
        .as_str()
        .unwrap()
        .contains("store.example.com"));
    let (plan, _) = api
        .json(api.get("/api/v1/config/plan"), StatusCode::OK)
        .await;
    assert!(!plan["resources"].as_array().unwrap().is_empty());
    let (dry, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/config/dry-run", "dry-run"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(dry["draft"]["pending"], true);
    let (applied, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/config/apply", "apply-text")
                .json(&json!({"note": "rename the shop"})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(applied["revision"], 2);

    let (revisions, _) = api
        .json(api.get("/api/v1/revisions?limit=10"), StatusCode::OK)
        .await;
    assert_eq!(revisions["items"][0]["note"], "rename the shop");
    assert_eq!(revisions["items"][1]["outcome"], "superseded");
    let (diff, _) = api
        .json(
            api.get("/api/v1/revisions/2/diff?against=1"),
            StatusCode::OK,
        )
        .await;
    assert!(diff["files"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("+        server_name store.example.com;"));
    let (noted, _) = api
        .json(
            api.mutate(Method::PUT, "/api/v1/revisions/1/note", "note")
                .json(&json!({"note": "first"})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(noted["note"], "first");
    let (restored, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/revisions/1/restore", "rollback"),
            StatusCode::OK,
        )
        .await;
    let (first, _) = api
        .json(api.get("/api/v1/revisions/1"), StatusCode::OK)
        .await;
    assert_eq!(restored["files"], first["files"]);
    let missing = api.get("/api/v1/revisions/99").send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let reload = api
        .mutate(Method::POST, "/api/v1/gateway/reload", "reload")
        .header("x-request-id", "req-audit-reload")
        .send()
        .await
        .unwrap();
    assert!(!reload.status().is_success());
    let audited = |correlation: &'static str, until: &'static str| {
        let api = &api;
        async move {
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let (page, _) = api
                        .json(
                            api.get(&format!(
                                "/api/v1/audit-events?correlation_id={correlation}"
                            )),
                            StatusCode::OK,
                        )
                        .await;
                    let items = page["items"].as_array().unwrap().clone();
                    if items.iter().any(|item| item["event_type"] == until) {
                        return items;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .expect("the request is audited")
        }
    };
    let applied = audited("req-audit-apply", "config.draft.applied").await;
    for expected in [
        "gateway.snapshot.prepared",
        "gateway.snapshot.activated",
        "config.draft.applied",
    ] {
        assert!(
            applied.iter().any(|item| item["event_type"] == expected),
            "{expected}: {applied:?}"
        );
    }
    assert!(applied.iter().all(|item| item["actor_id"] == "operator"));
    let refused = audited("req-audit-reload", "gateway.operation.refused").await;
    assert_eq!(refused[0]["data"]["operation"], "reloaded");
    assert_eq!(refused[0]["source"], "/pingora-panel/panel-api");

    let (page, _) = api
        .json(
            api.get("/api/v1/audit-events?type=config.&limit=2"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let next = page["next_before"].as_u64().unwrap();
    let (first, _) = api
        .json(api.get("/api/v1/audit-events/1"), StatusCode::OK)
        .await;
    assert_eq!(first["sequence"], 1);
    assert_eq!(first["previous_hash"], "");
    assert!(next > 1);
    let (verified, _) = api
        .json(api.get("/api/v1/audit-events/verify"), StatusCode::OK)
        .await;
    assert_eq!(verified["intact"], true);
    assert!(verified["checked"].as_u64().unwrap() > 5);
    let invalid = api
        .get("/api/v1/audit-events?since=yesterday")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    // Setting up and logging in are audited like every other change.
    let (identity, _) = api
        .json(
            api.get("/api/v1/audit-events?type=identity.&limit=10"),
            StatusCode::OK,
        )
        .await;
    let types: Vec<&str> = identity["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["event_type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"identity.login.succeeded"), "{types:?}");
    assert!(types.contains(&"identity.account.created"), "{types:?}");
    assert!(identity["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["actor_id"] == "operator"));

    // Security policies are named resources that sites use and the gateway
    // receives with the snapshot.
    let (policy, headers) = api
        .json(
            api.mutate(Method::PUT, "/api/v1/security-policies/office", "policy")
                .json(&json!({
                    "id": "office",
                    "allowed_cidrs": ["10.0.0.0/8"],
                    "rate_limits": [{"key": {"kind": "client_address"}, "requests": 10, "per_seconds": 1, "burst": 5}]
                })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(policy["rate_limits"][0]["burst"], 5);
    let policy_etag = headers["etag"].to_str().unwrap().to_owned();
    let (guarded, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/sites", "guarded-site")
                .json(&json!({
                    "name": "Intranet",
                    "action": {"type": "respond", "status": 204},
                    "domains": [{"host": "intranet.example.com", "primary": true}],
                    "security_policy_id": "office"
                })),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(guarded["security_policy_id"], "office");
    let (policies, _) = api
        .json(api.get("/api/v1/security-policies"), StatusCode::OK)
        .await;
    assert_eq!(policies[0]["used_by"][0], guarded["id"]);
    let in_use = api
        .mutate(
            Method::DELETE,
            "/api/v1/security-policies/office",
            "policy-delete",
        )
        .header("if-match", &policy_etag)
        .send()
        .await
        .unwrap();
    assert_eq!(in_use.status(), StatusCode::CONFLICT);
    let (source, _) = api
        .json(api.get("/api/v1/config/source"), StatusCode::OK)
        .await;
    assert!(
        source.to_string().contains("security_policy office {"),
        "{source}"
    );
    let (draft, _) = api
        .json(api.get("/api/v1/config/draft"), StatusCode::OK)
        .await;
    let (applied, _) = api
        .json(
            api.mutate(Method::POST, "/api/v1/config/apply", "apply-policy")
                .json(&json!({"expected_version": draft["version"]})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(applied["draft"]["pending"], false);

    stack.stop().await;
}

/// A client signed in as a new account with `roles`.
async fn person(api: &Api, username: &str, roles: &[&str]) -> Api {
    api.json(
        api.client
            .post(format!("{}/api/v1/accounts", api.base))
            .json(&json!({"username": username, "password": support::PASSWORD, "roles": roles})),
        StatusCode::CREATED,
    )
    .await;
    let (login, _) = api
        .json(
            Client::new()
                .post(format!("{}/api/v1/session", api.base))
                .json(&json!({"username": username, "password": support::PASSWORD, "transport": "bearer"})),
            StatusCode::CREATED,
        )
        .await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", login["secret"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let _ = rustls::crypto::ring::default_provider().install_default();
    Api {
        client: Client::builder().default_headers(headers).build().unwrap(),
        base: api.base.clone(),
    }
}

#[tokio::test]
async fn covered_changes_wait_for_another_person() {
    let Some(stack) = stack().await else {
        return;
    };
    use reqwest::Method;
    let admin = &stack.api;
    let bob = person(admin, "bob", &["operator"]).await;
    let (policy, _) = admin
        .json(
            admin
                .mutate(Method::PUT, "/api/v1/approval-policies/prod", "policy")
                .json(&json!({"site_tags": ["prod"], "description": "Production"})),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(policy["version"], 1);
    bob.json(
        bob.mutate(Method::PUT, "/api/v1/approval-policies/prod", "bob-policy")
            .json(&json!({})),
        StatusCode::FORBIDDEN,
    )
    .await;

    admin
        .json(
            admin
                .mutate(Method::PUT, "/api/v1/listeners/http", "listener")
                .json(&json!({"id": "http", "address": "0.0.0.0:8080"})),
            StatusCode::OK,
        )
        .await;
    admin
        .json(
            admin
                .mutate(Method::POST, "/api/v1/sites", "site")
                .json(&json!({
                    "name": "Shop",
                    "action": {"type": "respond"},
                    "domains": [{"host": "shop.example.com"}],
                    "tags": ["prod"]
                })),
            StatusCode::CREATED,
        )
        .await;
    let apply = |api: &Api, key: &str, body: Value| {
        api.mutate(Method::POST, "/api/v1/config/apply", key)
            .json(&body)
    };
    let (request, _) = admin
        .json(apply(admin, "apply-1", json!({})), StatusCode::ACCEPTED)
        .await;
    assert_eq!(
        (request["state"].clone(), request["requested_by"].clone()),
        (json!("pending"), json!("operator"))
    );
    let id = request["id"].as_str().unwrap();
    admin
        .json(
            admin.mutate(
                Method::POST,
                &format!("/api/v1/approvals/{id}/approve"),
                "self",
            ),
            StatusCode::FORBIDDEN,
        )
        .await;
    let (listed, _) = bob.json(bob.get("/api/v1/approvals"), StatusCode::OK).await;
    assert_eq!(listed["items"][0]["id"], id);
    let (approved, _) = bob
        .json(
            bob.mutate(
                Method::POST,
                &format!("/api/v1/approvals/{id}/approve"),
                "approve",
            ),
            StatusCode::OK,
        )
        .await;
    assert_eq!(approved["state"], "approved");
    let (applied, _) = admin
        .json(apply(admin, "apply-2", json!({})), StatusCode::OK)
        .await;
    assert!(applied["revision"].as_u64().is_some());

    admin
        .json(
            admin
                .mutate(Method::POST, "/api/v1/sites", "site-2")
                .json(&json!({
                    "name": "Checkout",
                    "action": {"type": "respond"},
                    "domains": [{"host": "checkout.example.com"}],
                    "tags": ["prod"]
                })),
            StatusCode::CREATED,
        )
        .await;
    let bypass =
        json!({"bypass": {"reason": "checkout is down for everyone", "incident": "INC-7"}});
    bob.json(
        apply(&bob, "bypass-bob", bypass.clone()),
        StatusCode::FORBIDDEN,
    )
    .await;
    admin
        .json(apply(admin, "bypass-admin", bypass), StatusCode::OK)
        .await;
    stack.stop().await;
}

#[tokio::test]
async fn grants_limit_people_to_a_site_group_and_their_conditions() {
    let Some(stack) = stack().await else {
        return;
    };
    use reqwest::Method;
    let admin = &stack.api;
    admin
        .json(
            admin
                .mutate(Method::PUT, "/api/v1/listeners/http", "listener")
                .json(&json!({"id": "http", "address": "0.0.0.0:8080"})),
            StatusCode::OK,
        )
        .await;
    let mut ids = HashMap::new();
    for (name, group) in [("shop", "shop"), ("intranet", "corp")] {
        let (site, _) = admin
            .json(
                admin
                    .mutate(Method::POST, "/api/v1/sites", name)
                    .json(&json!({
                        "name": name,
                        "group": group,
                        "action": {"type": "respond"},
                        "domains": [{"host": format!("{name}.example.com")}]
                    })),
                StatusCode::CREATED,
            )
            .await;
        ids.insert(name, site["id"].as_str().unwrap().to_owned());
    }
    admin
        .json(
            admin
                .mutate(Method::POST, "/api/v1/config/apply", "apply-1")
                .json(&json!({})),
            StatusCode::OK,
        )
        .await;

    let keeper = person(admin, "keeper", &[]).await;
    let (accounts, _) = admin
        .json(admin.get("/api/v1/accounts"), StatusCode::OK)
        .await;
    let keeper_id = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["username"] == "keeper")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (grant, _) = admin
        .json(
            admin
                .mutate(
                    Method::POST,
                    &format!("/api/v1/accounts/{keeper_id}/grants"),
                    "grant",
                )
                .json(
                    &json!({"role": "operator", "scope": {"kind": "site_group", "group": "shop"}}),
                ),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(grant["scope"]["group"], "shop");

    let (current, _) = keeper
        .json(keeper.get("/api/v1/session"), StatusCode::OK)
        .await;
    assert!(current["limited"]
        .as_array()
        .unwrap()
        .contains(&json!("config.write")));
    let (sites, _) = keeper
        .json(keeper.get("/api/v1/sites"), StatusCode::OK)
        .await;
    let names: Vec<&str> = sites["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| site["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["shop"]);
    keeper
        .json(
            keeper.mutate(
                Method::POST,
                &format!("/api/v1/sites/{}/disable", ids["intranet"]),
                "theirs",
            ),
            StatusCode::FORBIDDEN,
        )
        .await;
    keeper
        .json(
            keeper
                .mutate(Method::PUT, "/api/v1/listeners/https", "shared")
                .json(&json!({"id": "https", "address": "0.0.0.0:8443"})),
            StatusCode::FORBIDDEN,
        )
        .await;
    keeper
        .json(
            keeper.mutate(
                Method::POST,
                &format!("/api/v1/sites/{}/disable", ids["shop"]),
                "mine",
            ),
            StatusCode::OK,
        )
        .await;
    keeper
        .json(
            keeper
                .mutate(Method::POST, "/api/v1/config/apply", "apply-2")
                .json(&json!({})),
            StatusCode::OK,
        )
        .await;

    let remote = person(admin, "remote", &[]).await;
    let remote_id = admin
        .json(admin.get("/api/v1/accounts"), StatusCode::OK)
        .await
        .0
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["username"] == "remote")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    admin
        .json(
            admin
                .mutate(
                    Method::POST,
                    &format!("/api/v1/accounts/{remote_id}/grants"),
                    "office-only",
                )
                .json(&json!({"role": "viewer", "conditions": {"networks": ["203.0.113.0/24"]}})),
            StatusCode::CREATED,
        )
        .await;
    remote
        .json(remote.get("/api/v1/sites"), StatusCode::FORBIDDEN)
        .await;
    stack.stop().await;
}
