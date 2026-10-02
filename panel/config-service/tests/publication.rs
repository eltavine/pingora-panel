#![forbid(unsafe_code)]

use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc::GatewayGrpcService;
use panel_application::{
    CommandContext, ConfigDocument, DeploymentOutcome, GatewayUseCases, IdempotencyKey,
    IdempotencyLookup, RequestDeadline, RequestId, RequestScope,
};
use panel_control_runtime::{
    ProcessSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_domain::RevisionId;
use panel_engine::FakeGatewayEngine;
use panel_errors::ErrorCode;
use panel_health::HealthStatus;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_service::Environment;
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

fn document(revision: u64) -> ConfigDocument {
    ConfigDocument::new(
        IR_SCHEMA_VERSION,
        "application/json",
        serde_json::to_vec(&RuntimeSnapshot::empty(RevisionId::new(revision))).unwrap(),
    )
    .unwrap()
}

fn command(request: &str, key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(request).unwrap(),
        RequestId::new("flow-1").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn publication_prepares_activates_and_replays_receipts() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let gateway = fake_gateway().await;
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
    let status = client
        .status_with_scope(RequestScope::new(RequestId::new("status-1").unwrap()))
        .await
        .unwrap();
    assert!(status.ready());
    assert!(client.validate(document(1)).await.unwrap().valid);

    let prepared = client
        .prepare(command("prepare-1", "prepare-1"), document(1))
        .await
        .unwrap();
    let activation = command("activate-1", "activate-1");
    let activated = client
        .activate(activation.clone(), prepared.prepare_token().into(), None)
        .await
        .unwrap();
    assert_eq!(activated.revision_id(), RevisionId::new(1));
    let replayed = client
        .activate(activation, prepared.prepare_token().into(), None)
        .await
        .unwrap();
    assert_eq!(
        replayed, activated,
        "a retried activation replays its receipt"
    );

    let receipt = client
        .activation_receipt(&IdempotencyKey::new("activate-1").unwrap())
        .await
        .unwrap();
    let IdempotencyLookup::Completed(record) = receipt else {
        panic!("the activation receipt is recorded");
    };
    assert_eq!(record.outcome(), &DeploymentOutcome::Succeeded(activated));
    assert_eq!(
        client
            .activation_receipt(&IdempotencyKey::new("unknown").unwrap())
            .await
            .unwrap(),
        IdempotencyLookup::Missing
    );

    let conflict = client
        .activate(
            command("activate-2", "activate-1"),
            "other-token".into(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code.as_str(), ErrorCode::CONFLICT);

    process.stop().await;
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
    database.drop().await;
    broker.drop().await;
}
