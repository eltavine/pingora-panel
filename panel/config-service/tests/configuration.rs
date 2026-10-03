#![forbid(unsafe_code)]

//! Edits the draft through the configuration API and applies it to a gateway.

use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc::GatewayGrpcService;
use panel_application::{
    ApplyOutcome, CommandContext, ConfigurationChange, ConfigurationPort, ConfigurationRead,
    IdempotencyKey, RequestDeadline, RequestId, RequestScope,
};
use panel_control_runtime::{
    ProcessSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_errors::ErrorCode;
use panel_health::HealthStatus;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_service::Environment;
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

const CAPABILITIES: &[&str] = &[
    "action.redirect",
    "action.respond",
    "action.static",
    "activation.cas",
    "listener.http",
    "listener.http2",
    "listener.https",
    "route.exact-path",
    "route.glob",
    "route.host",
    "route.path-prefix",
    "route.regex",
    "site.redirect",
    "upstream.backup",
    "upstream.balancing",
    "upstream.health-check",
    "upstream.http",
    "upstream.http2",
    "upstream.https",
    "upstream.passive-health",
];

async fn gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let engine = FakeGatewayEngine::new(
        CAPABILITIES
            .iter()
            .map(|name| EngineCapability::new(*name, "1")),
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

fn command(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("flow-1").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

fn change(operation: &str, resource: &str, body: Value) -> ConfigurationChange {
    ConfigurationChange {
        operation: operation.into(),
        resource: resource.into(),
        if_match: None,
        content: serde_json::to_vec(&body).unwrap(),
    }
}

fn json(content: &[u8]) -> Value {
    serde_json::from_slice(content).unwrap()
}

#[tokio::test]
async fn draft_changes_are_idempotent_conditional_and_applied() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let gateway = gateway().await;
    let values: HashMap<&str, OsString> = HashMap::from([
        (DATABASE_URL_ENV, database.service_url("config").into()),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
        (
            NATS_URL_ENV,
            std::env::var(TEST_NATS_URL_ENV).unwrap().into(),
        ),
        (
            config_service::GATEWAY_URL_ENV,
            format!("http://{gateway}").into(),
        ),
    ]);
    let mut env = Environment::from_lookup(move |name| values.get(name).cloned());
    let settings = ProcessSettings::read(&mut env, config_service::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50));
    let process = config_service::process(&mut env, settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().status() != HealthStatus::Pass {
            assert!(health.changed().await);
        }
    })
    .await
    .expect("config-service becomes ready");
    let client = ConfigPublicationClient::connect_lazy(
        format!("http://{}", process.grpc_address()),
        ConfigClientConfig::default(),
    )
    .unwrap();
    let scope = || RequestScope::new(RequestId::new("read").unwrap());

    let summary = client
        .read(
            scope(),
            ConfigurationRead {
                operation: "sites.summary".into(),
                resource: "sites".into(),
                parameters: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(json(&summary.content)["total"], 0);
    assert_eq!(summary.draft.version, 0);
    assert_eq!(
        client
            .apply(command("apply-empty"), 0)
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::PRECONDITION_FAILED
    );

    let upstream = client
        .change(
            command("upstream"),
            change(
                "upstreams.create",
                "upstreams",
                json!({"name": "app", "nodes": [{"host": "127.0.0.1", "port": 8080}]}),
            ),
        )
        .await
        .unwrap();
    let upstream_id = json(&upstream.content)["id"].as_str().unwrap().to_owned();
    let listener = change(
        "listeners.put",
        "listeners/http",
        json!({"id": "http", "address": "0.0.0.0:80"}),
    );
    client.change(command("listener"), listener).await.unwrap();
    let create = change(
        "sites.create",
        "sites",
        json!({
            "name": "shop",
            "action": {"type": "proxy", "upstream_id": upstream_id},
            "domains": [{"host": "shop.example.com"}]
        }),
    );
    let site = client
        .change(command("site"), create.clone())
        .await
        .unwrap();
    assert_eq!(site.draft.version, 3);
    let replayed = client.change(command("site"), create).await.unwrap();
    assert_eq!(replayed.content, site.content);
    assert_eq!(replayed.draft.version, 3);
    let reused = client
        .change(
            command("site"),
            change(
                "sites.create",
                "sites",
                json!({"name": "other", "action": {"type": "respond"}}),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(reused.code.as_str(), ErrorCode::CONFLICT);

    let site = json(&site.content);
    let resource = format!("sites/{}", site["id"].as_str().unwrap());
    let mut stale = change("sites.disable", &resource, Value::Null);
    stale.if_match = Some("\"stale\"".into());
    assert_eq!(
        client
            .change(command("stale"), stale)
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::PRECONDITION_FAILED
    );
    let duplicate = client
        .change(
            command("duplicate"),
            change(
                "sites.create",
                "sites",
                json!({"name": "copy", "action": {"type": "respond"}, "domains": [{"host": "SHOP.example.com"}]}),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate.code.as_str(), ErrorCode::VALIDATION_FAILED);
    assert!(duplicate.diagnostics[0].message.contains("already bound"));

    match client.apply(command("apply-1"), 3).await.unwrap() {
        ApplyOutcome::Applied { draft, deployment } => {
            assert_eq!(draft.applied_version, Some(3));
            assert!(!draft.pending());
            assert_eq!(deployment.revision_id().get(), 3);
        }
        other => panic!("expected an applied draft, got {other:?}"),
    }
    assert_eq!(
        client
            .apply(command("apply-2"), 2)
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::CONFLICT
    );
    let listed = client
        .read(
            scope(),
            ConfigurationRead {
                operation: "sites.list".into(),
                resource: "sites".into(),
                parameters: serde_json::to_vec(&json!({"q": "shop"})).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(json(&listed.content)["items"][0]["status"], "running");
    assert_eq!(listed.draft.applied_version, Some(3));
}
