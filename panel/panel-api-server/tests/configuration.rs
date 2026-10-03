#![forbid(unsafe_code)]

//! Configuration resources through the public HTTP API, from creation to
//! applying the draft on a gateway.

use gateway_grpc::GatewayGrpcService;
use panel_control_runtime::{
    ProcessSettings, RunningProcess, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_health::HealthStatus;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
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

async fn ready(process: &RunningProcess) {
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().status() != HealthStatus::Pass {
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
            .header("x-actor", "operator")
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

#[tokio::test]
async fn sites_are_edited_validated_and_applied_through_the_api() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database
        .bootstrap(&[("config", "config"), ("identity", "identity")])
        .await;
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let gateway = gateway().await;
    let mut config_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("config")),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
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
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<!doctype html>").unwrap();
    let http = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut api_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("identity")),
        (DATABASE_PASSWORD_ENV, secrets[1].expose().into()),
        (NATS_URL_ENV, nats),
        (panel_api_server::HTTP_ADDRESS_ENV, http.to_string()),
        (
            panel_api_server::CONFIG_URL_ENV,
            format!("http://{}", config.grpc_address()),
        ),
        (
            panel_api_server::WEB_ROOT_ENV,
            web.path().display().to_string(),
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
    let api = Api {
        client: Client::new(),
        base: format!("http://{http}"),
    };
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

    server.stop().await;
    config.stop().await;
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
    database.drop().await;
    broker.drop().await;
}
