#![forbid(unsafe_code)]

use chrono::Utc;
use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc::GatewayGrpcService;
use panel_application::{
    CommandContext, ConfigDocument, GatewayUseCases, IdempotencyKey, RequestDeadline, RequestId,
};
use panel_context::ServiceName;
use panel_control_runtime::{
    ProcessSettings, RunningProcess, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
    TLS_DIR_ENV,
};
use panel_domain::RevisionId;
use panel_engine::FakeGatewayEngine;
use panel_health::HealthStatus;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_pki::{
    CertificateAuthority, CredentialFiles, IssuanceTarget, TrustDomain, WorkloadIdentity,
    DEFAULT_AUTHORITY_VALIDITY,
};
use panel_postgres::testing::TestDatabase;
use panel_service::Environment;
use panel_tls::{PeerPolicy, TlsCredentials};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tonic::transport::Server;

const SERVICES: [&str; 4] = [
    "panel-api",
    "config-service",
    "gatewayd",
    "automation-service",
];

fn issue(root: &Path) -> HashMap<&'static str, PathBuf> {
    let (authority, _) = CertificateAuthority::load_or_create(
        &root.join("authority"),
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        Utc::now(),
    )
    .unwrap();
    let targets: Vec<IssuanceTarget> = SERVICES
        .iter()
        .map(|service| IssuanceTarget {
            service: ServiceName::new(*service).unwrap(),
            files: CredentialFiles::new(root.join(service)),
            alternative_names: vec![(*service).to_owned()],
        })
        .collect();
    authority
        .renew_due(&targets, Duration::from_secs(3600), Utc::now())
        .unwrap();
    SERVICES
        .iter()
        .map(|service| (*service, root.join(service)))
        .collect()
}

fn credentials(directory: &Path, service: &str) -> Arc<TlsCredentials> {
    TlsCredentials::load(
        CredentialFiles::new(directory),
        WorkloadIdentity::new(ServiceName::new(service).unwrap(), TrustDomain::default()),
    )
    .unwrap()
}

async fn tls_gateway(directory: &Path) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let gateway = GatewayGrpcService::new(Arc::new(FakeGatewayEngine::with_default_capabilities()));
    let policy = PeerPolicy::new(TrustDomain::default()).allow(
        "pingora.panel.gateway.v1.GatewayEngine",
        [ServiceName::new("config-service").unwrap()],
    );
    tokio::spawn(
        Server::builder()
            .layer(policy)
            .add_service(health)
            .add_service(gateway.transport_policy().gateway_server(gateway))
            .serve_with_incoming(panel_tls::incoming(
                listener,
                credentials(directory, "gatewayd"),
                Duration::from_secs(5),
            )),
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

async fn ready(process: &RunningProcess) {
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().status() != HealthStatus::Pass {
            assert!(health.changed().await);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("not ready: {:?}", process.health().current()));
}

fn command(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("flow").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn every_internal_hop_is_mutually_authenticated() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let root = std::env::temp_dir().join(format!("panel-mtls-stack-{}", std::process::id()));
    let directories = issue(&root);
    let secrets = database
        .bootstrap(&[("config", "config"), ("identity", "identity")])
        .await;
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let gateway = tls_gateway(&directories["gatewayd"]).await;

    let mut config_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("config")),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
        (NATS_URL_ENV, nats.clone()),
        (
            TLS_DIR_ENV,
            directories["config-service"].display().to_string(),
        ),
        (
            config_service::GATEWAY_URL_ENV,
            format!("https://{gateway}"),
        ),
    ]);
    let config_settings =
        ProcessSettings::read(&mut config_env, config_service::default_addresses())
            .unwrap()
            .with_listeners(
                "127.0.0.1:0".parse().unwrap(),
                "127.0.0.1:0".parse().unwrap(),
            )
            .with_health_interval(Duration::from_millis(50));
    let config = config_service::process(&mut config_env, config_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&config).await;

    let http = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut api_env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("identity")),
        (DATABASE_PASSWORD_ENV, secrets[1].expose().into()),
        (NATS_URL_ENV, nats),
        (TLS_DIR_ENV, directories["panel-api"].display().to_string()),
        (panel_api_server::HTTP_ADDRESS_ENV, http.to_string()),
        (
            panel_api_server::CONFIG_URL_ENV,
            format!("https://{}", config.grpc_address()),
        ),
        (
            panel_api_server::WEB_ROOT_ENV,
            root.join("no-console").display().to_string(),
        ),
    ]);
    let api_settings = ProcessSettings::read(&mut api_env, panel_api_server::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50));
    let api = panel_api_server::process(&mut api_env, api_settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    ready(&api).await;

    let client = reqwest::Client::new();
    let base = format!("http://{http}");
    let mutation = |path: &str, key: &str| {
        client
            .post(format!("{base}{path}"))
            .header("x-actor", "operator")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", key)
    };
    let prepared: Value = mutation("/api/v1/gateway/prepare", "prepare-1")
        .json(&json!({
            "schema_version": IR_SCHEMA_VERSION,
            "snapshot": RuntimeSnapshot::empty(RevisionId::new(1)),
        }))
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

    // A service the policy does not list cannot publish.
    let intruder = ConfigPublicationClient::from_channel(
        panel_tls::channel(
            &config.grpc_address().to_string(),
            &WorkloadIdentity::new(
                ServiceName::new("config-service").unwrap(),
                TrustDomain::default(),
            ),
            credentials(&directories["automation-service"], "automation-service"),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .unwrap(),
        ConfigClientConfig::default(),
    );
    let refused = intruder
        .prepare(
            command("intruder"),
            ConfigDocument::new(IR_SCHEMA_VERSION, "application/json", b"{}".to_vec()).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.code.as_str(),
        panel_errors::ErrorCode::PERMISSION_DENIED
    );

    api.stop().await;
    config.stop().await;
    std::fs::remove_dir_all(root).unwrap();
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
    database.drop().await;
    broker.drop().await;
}
