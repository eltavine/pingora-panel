#![forbid(unsafe_code)]

use gateway_grpc::GatewayGrpcService;
use panel_control_runtime::{
    ProcessSettings, RunningProcess, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_domain::RevisionId;
use panel_engine::FakeGatewayEngine;
use panel_health::HealthStatus;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_service::Environment;
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

async fn fake_gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let gateway = GatewayGrpcService::new(Arc::new(FakeGatewayEngine::with_default_capabilities()));
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

fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

#[tokio::test]
async fn the_public_api_publishes_through_config_service_and_serves_the_console() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database
        .bootstrap(&[("config", "config"), ("identity", "identity")])
        .await;
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let gateway = fake_gateway().await;

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

    let web = std::env::temp_dir().join(format!("panel-web-{}", std::process::id()));
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(
        web.join("index.html"),
        "<!doctype html><title>Pingora Panel</title>",
    )
    .unwrap();
    std::fs::write(web.join("assets/app.js"), "export {}").unwrap();
    let http = free_port();
    let mut api_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("identity")),
        (DATABASE_PASSWORD_ENV, secrets[1].expose().into()),
        (NATS_URL_ENV, nats),
        (panel_api_server::HTTP_ADDRESS_ENV, http.to_string()),
        (
            panel_api_server::CONFIG_URL_ENV,
            format!("http://{}", config.grpc_address()),
        ),
        (panel_api_server::WEB_ROOT_ENV, web.display().to_string()),
    ]);
    let api_settings =
        local(ProcessSettings::read(&mut api_env, panel_api_server::default_addresses()).unwrap());
    let api = panel_api_server::process(&mut api_env, api_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&api).await;

    let client = reqwest::Client::new();
    let base = format!("http://{http}");
    let status: Value = client
        .get(format!("{base}/api/v1/gateway/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["ready"], true);

    let document = json!({
        "schema_version": IR_SCHEMA_VERSION,
        "snapshot": RuntimeSnapshot::empty(RevisionId::new(1)),
    });
    let mutation = |path: &str, key: &str| {
        client
            .post(format!("{base}{path}"))
            .header("x-actor", "operator")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", key)
    };
    let prepared: Value = mutation("/api/v1/gateway/prepare", "prepare-1")
        .json(&document)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let activated = mutation("/api/v1/gateway/activate", "activate-1")
        .json(&json!({ "prepare_token": prepared["prepare_token"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(activated.status(), 200);
    let activated: Value = activated.json().await.unwrap();
    assert_eq!(activated["revision_id"], 1);

    let mut listed = Vec::new();
    for _ in 0..100 {
        let listing: Value = client
            .get(format!("{base}/api/v1/platform/services"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        listed = listing["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|service| service["service"].as_str().unwrap().to_owned())
            .collect();
        if listed.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(listed, ["config-service", "panel-api"]);

    let console = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(console.status(), 200);
    assert!(console.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
    assert_eq!(console.headers()["x-content-type-options"], "nosniff");
    assert!(console.text().await.unwrap().contains("Pingora Panel"));
    let route = client
        .get(format!("{base}/gateway/receipts"))
        .send()
        .await
        .unwrap();
    assert!(route.text().await.unwrap().contains("Pingora Panel"));
    let asset = client
        .get(format!("{base}/assets/app.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.text().await.unwrap(), "export {}");
    let missing = client
        .get(format!("{base}/api/v1/missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.headers()["content-type"],
        "application/problem+json"
    );

    api.stop().await;
    config.stop().await;
    assert!(tokio::net::TcpStream::connect(http).await.is_err());
    std::fs::remove_dir_all(web).unwrap();
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
    database.drop().await;
    broker.drop().await;
}
