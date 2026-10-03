#![forbid(unsafe_code)]

//! The certificate inventory through the public HTTP API: certificates are
//! kept by `automation-service`, delivered to the gateway's secret
//! directory and audited.

mod support;

use chrono::Utc;
use gateway_grpc::GatewayGrpcService;
use panel_certificates::self_signed;
use panel_control_runtime::{
    ProcessSettings, RunningProcess, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_health::ServiceMode;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_secrets::EnvelopeVault;
use panel_service::Environment;
use reqwest::{Client, Method, RequestBuilder, StatusCode};
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
        ["activation.cas", "listener.http"].map(|name| EngineCapability::new(name, "1")),
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

    fn mutate(&self, method: Method, path: &str, key: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", key)
    }

    async fn send(&self, request: RequestBuilder, expected: StatusCode) -> (Value, String) {
        let response = request.send().await.unwrap();
        let status = response.status();
        let etag = response
            .headers()
            .get("etag")
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let text = response.text().await.unwrap();
        assert_eq!(status, expected, "{text}");
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap()
        };
        (body, etag)
    }
}

#[tokio::test]
async fn certificates_are_kept_delivered_and_audited() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database
        .bootstrap(&[
            ("config", "config"),
            ("identity", "identity"),
            ("audit", "audit"),
            ("automation", "automation"),
        ])
        .await;
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let gateway = gateway().await;
    let start = |process: panel_control_runtime::ControlPlaneProcess| {
        process
            .with_jetstream_settings((*broker.settings).clone())
            .start()
    };

    let mut config_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("config")),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
        (NATS_URL_ENV, nats.clone()),
        (config_service::GATEWAY_URL_ENV, format!("http://{gateway}")),
    ]);
    let settings =
        local(ProcessSettings::read(&mut config_env, config_service::default_addresses()).unwrap());
    let config = start(config_service::process(&mut config_env, settings).unwrap())
        .await
        .unwrap();
    ready(&config).await;

    let mut audit_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("audit")),
        (DATABASE_PASSWORD_ENV, secrets[2].expose().into()),
        (NATS_URL_ENV, nats.clone()),
    ]);
    let settings =
        local(ProcessSettings::read(&mut audit_env, audit_service::default_addresses()).unwrap());
    let audit = start(audit_service::process(&mut audit_env, settings).unwrap())
        .await
        .unwrap();
    ready(&audit).await;

    let directory = tempfile::tempdir().unwrap();
    let mut automation_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("automation")),
        (DATABASE_PASSWORD_ENV, secrets[3].expose().into()),
        (NATS_URL_ENV, nats.clone()),
        (
            automation_service::MASTER_KEYS_ENV,
            EnvelopeVault::generate_key().unwrap(),
        ),
        (
            automation_service::GATEWAY_SECRET_DIR_ENV,
            directory.path().display().to_string(),
        ),
    ]);
    let settings = local(
        ProcessSettings::read(&mut automation_env, automation_service::default_addresses())
            .unwrap(),
    );
    let automation = start(automation_service::process(&mut automation_env, settings).unwrap())
        .await
        .unwrap();
    ready(&automation).await;

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
            panel_api_server::BOOTSTRAP_TOKEN_ENV,
            support::BOOTSTRAP.into(),
        ),
        (
            panel_api_server::CONFIG_URL_ENV,
            format!("http://{}", config.grpc_address()),
        ),
        (
            panel_api_server::AUDIT_URL_ENV,
            format!("http://{}", audit.grpc_address()),
        ),
        (
            panel_api_server::AUTOMATION_URL_ENV,
            format!("http://{}", automation.grpc_address()),
        ),
        (
            panel_api_server::WEB_ROOT_ENV,
            web.path().display().to_string(),
        ),
    ]);
    let settings =
        local(ProcessSettings::read(&mut api_env, panel_api_server::default_addresses()).unwrap());
    let server = start(panel_api_server::process(&mut api_env, settings).unwrap())
        .await
        .unwrap();
    ready(&server).await;
    let base = format!("http://{http}");
    let api = Api {
        client: support::signed_in(&base).await,
        base,
    };

    let (generated, etag) = api
        .send(
            api.mutate(Method::POST, "/api/v1/certificates", "generate-1")
                .json(&json!({
                    "source": "self_signed",
                    "id": "intranet",
                    "names": ["intranet.example", "*.intranet.example"],
                    "days": 90,
                })),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(etag, "\"1\"");
    assert_eq!(generated["source"], "self_signed");
    assert_eq!(generated["status"], "valid");
    let key_file = directory.path().join("cert-intranet.key");
    assert!(std::fs::read_to_string(&key_file)
        .unwrap()
        .starts_with("-----BEGIN PRIVATE KEY-----"));
    assert!(
        std::fs::read_to_string(directory.path().join("cert-intranet.pem"))
            .unwrap()
            .starts_with("-----BEGIN CERTIFICATE-----")
    );

    let material = self_signed(&["shop.example".into()], 30, Utc::now()).unwrap();
    let (uploaded, _) = api
        .send(
            api.mutate(Method::POST, "/api/v1/certificates", "upload-1")
                .json(&json!({
                    "source": "upload",
                    "id": "shop.example",
                    "chain": material.chain,
                    "key": *material.key,
                })),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(uploaded["fingerprint"], material.details.fingerprint);
    assert_eq!(uploaded["status"], "expiring");
    assert!(!uploaded.to_string().contains("PRIVATE KEY"));

    let (listed, _) = api
        .send(api.get("/api/v1/certificates"), StatusCode::OK)
        .await;
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|certificate| certificate["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["intranet", "shop.example"]);
    let (coverage, _) = api
        .send(
            api.get(
                "/api/v1/certificates/intranet/coverage?hosts=www.intranet.example,shop.example",
            ),
            StatusCode::OK,
        )
        .await;
    assert_eq!(coverage["hosts"][0]["covered"], true);
    assert_eq!(coverage["hosts"][1]["covered"], false);

    let renewed = self_signed(&["shop.example".into()], 60, Utc::now()).unwrap();
    let replace = json!({ "chain": renewed.chain, "key": *renewed.key });
    api.send(
        api.mutate(
            Method::PUT,
            "/api/v1/certificates/shop.example",
            "replace-0",
        )
        .json(&replace),
        StatusCode::PRECONDITION_REQUIRED,
    )
    .await;
    let (replaced, etag) = api
        .send(
            api.mutate(
                Method::PUT,
                "/api/v1/certificates/shop.example",
                "replace-1",
            )
            .header("if-match", "\"1\"")
            .json(&replace),
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        (replaced["version"].as_u64(), etag.as_str()),
        (Some(2), "\"2\"")
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("cert-shop.example.key")).unwrap(),
        *renewed.key
    );
    api.send(
        api.mutate(Method::POST, "/api/v1/certificates", "upload-2")
            .json(&json!({
                "source": "upload",
                "id": "broken",
                "chain": material.chain,
                "key": *renewed.key,
            })),
        StatusCode::BAD_REQUEST,
    )
    .await;
    api.send(
        api.mutate(Method::DELETE, "/api/v1/certificates/intranet", "delete-1")
            .header("if-match", "\"1\""),
        StatusCode::NO_CONTENT,
    )
    .await;
    assert!(!key_file.exists());
    api.send(
        api.get("/api/v1/certificates/intranet"),
        StatusCode::NOT_FOUND,
    )
    .await;

    let audited = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let (page, _) = api
                .send(
                    api.get("/api/v1/audit-events?type=tls.certificate."),
                    StatusCode::OK,
                )
                .await;
            let items: Vec<(String, String)> = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    (
                        item["event_type"].as_str().unwrap().to_owned(),
                        item["actor_id"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            if items.len() >= 5 {
                return items;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("certificate changes are audited");
    let types: Vec<&str> = audited.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        types,
        [
            "tls.certificate.deleted",
            "tls.certificate.refused",
            "tls.certificate.replaced",
            "tls.certificate.created",
            "tls.certificate.created",
        ]
    );
    assert!(audited.iter().all(|(_, actor)| actor == "operator"));

    server.stop().await;
    automation.stop().await;
    audit.stop().await;
    config.stop().await;
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
}
