#![forbid(unsafe_code)]

use config_service::{
    PgActivationReceipts, PgDeployments, Reconciler, Reconciliation, ReconciliationCheck,
    RecordingUseCases, MIGRATIONS,
};
use gateway_grpc::GatewayGrpcService;
use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use panel_application::{
    CommandContext, ConfigDocument, ContentHash, DeploymentOutcome, GatewayService,
    GatewayUseCases, IdempotencyKey, IdempotencyLookup, IdempotencyRepository,
    IdempotentGatewayUseCases, RequestDeadline, RequestId,
};
use panel_config_json::{JsonCompilerConfig, JsonRuntimeSnapshotCompiler};
use panel_domain::RevisionId;
use panel_engine::FakeGatewayEngine;
use panel_errors::ErrorCode;
use panel_health::{HealthCheck, HealthStatus};
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use panel_postgres::{testing::TestDatabase, ServiceDatabase};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

/// A fresh in-memory gateway behind the real gRPC adapter and client.
async fn gateway() -> Arc<dyn GatewayUseCases> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = GatewayGrpcService::new(Arc::new(FakeGatewayEngine::with_default_capabilities()));
    tokio::spawn(
        Server::builder()
            .add_service(service.transport_policy().gateway_server(service))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let client = GatewayGrpcClient::connect_lazy(
        format!("http://{address}"),
        GatewayGrpcClientConfig::default(),
    )
    .unwrap();
    Arc::new(GatewayService::new(
        Arc::new(client),
        Arc::new(JsonRuntimeSnapshotCompiler::new(JsonCompilerConfig::default()).unwrap()),
    ))
}

fn document(revision: u64) -> ConfigDocument {
    ConfigDocument::new(
        IR_SCHEMA_VERSION,
        "application/json",
        serde_json::to_vec(&RuntimeSnapshot::empty(RevisionId::new(revision))).unwrap(),
    )
    .unwrap()
}

fn context(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("flow-1").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

struct Fixture {
    database: TestDatabase,
    service: ServiceDatabase,
    receipts: Arc<PgActivationReceipts>,
    deployments: PgDeployments,
}

async fn fixture() -> Option<Fixture> {
    let mut database = TestDatabase::create().await?;
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let service = database
        .connect_service("config", "config", &secrets[0])
        .await;
    service.migrate(MIGRATIONS).await.unwrap();
    Some(Fixture {
        receipts: Arc::new(PgActivationReceipts::new(&service)),
        deployments: PgDeployments::new(&service),
        service,
        database,
    })
}

impl Fixture {
    fn reconciler(&self, gateway: &Arc<dyn GatewayUseCases>) -> (Reconciler, RecordingUseCases) {
        let (reconciler, watch) = Reconciler::new(
            Arc::clone(gateway),
            Arc::clone(&self.receipts),
            self.deployments.clone(),
        );
        let publication = RecordingUseCases::new(
            Arc::new(IdempotentGatewayUseCases::new(
                Arc::clone(gateway),
                Arc::clone(&self.receipts) as Arc<dyn IdempotencyRepository>,
            )),
            self.deployments.clone(),
            watch,
        );
        (reconciler, publication)
    }

    /// An activation that claimed its key and stopped before its receipt.
    async fn interrupted(&self, key: &str, token: &str, expected: Option<&ContentHash>) {
        self.deployments
            .record_intent(&context(key), token, expected)
            .await
            .unwrap();
        self.receipts
            .claim(
                &IdempotencyKey::new(key).unwrap(),
                &ContentHash::from_bytes(key.as_bytes()),
            )
            .await
            .unwrap();
    }

    async fn lookup(&self, key: &str) -> IdempotencyLookup {
        self.receipts
            .lookup(&IdempotencyKey::new(key).unwrap())
            .await
            .unwrap()
    }

    async fn finish(self) {
        self.service.close().await;
        self.database.drop().await;
    }
}

#[tokio::test]
async fn interrupted_activations_are_completed_or_released() {
    let Some(fixture) = fixture().await else {
        return;
    };
    let gateway = gateway().await;
    let (reconciler, publication) = fixture.reconciler(&gateway);

    // Stopped before reaching the gateway: reconciliation activates it.
    let first = publication
        .prepare(context("prepare-1"), document(1))
        .await
        .unwrap();
    fixture
        .interrupted("activate-1", first.prepare_token(), None)
        .await;
    assert_eq!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::InSync
    );
    let IdempotencyLookup::Completed(record) = fixture.lookup("activate-1").await else {
        panic!("the interrupted activation is completed");
    };
    let DeploymentOutcome::Succeeded(activated) = record.outcome() else {
        panic!("the activation succeeded");
    };
    assert_eq!(activated.revision_id(), RevisionId::new(1));
    assert_eq!(
        fixture
            .deployments
            .desired()
            .await
            .unwrap()
            .unwrap()
            .prepare_token,
        first.prepare_token()
    );

    // Stopped after the gateway committed: the gateway replays its receipt.
    let second = publication
        .prepare(context("prepare-2"), document(2))
        .await
        .unwrap();
    gateway
        .activate(
            context("direct-2"),
            second.prepare_token().into(),
            Some(first.content_hash().clone()),
        )
        .await
        .unwrap();
    fixture
        .interrupted(
            "activate-2",
            second.prepare_token(),
            Some(first.content_hash()),
        )
        .await;
    reconciler.reconcile().await.unwrap();
    assert!(matches!(
        fixture.lookup("activate-2").await,
        IdempotencyLookup::Completed(_)
    ));
    assert_eq!(
        fixture
            .deployments
            .desired()
            .await
            .unwrap()
            .unwrap()
            .revision_id,
        RevisionId::new(2)
    );

    // Unable to commit any more: the claim is released for a retry.
    let third = publication
        .prepare(context("prepare-3"), document(3))
        .await
        .unwrap();
    fixture
        .interrupted(
            "activate-3",
            third.prepare_token(),
            Some(&ContentHash::from_bytes(b"stale")),
        )
        .await;
    assert_eq!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::InSync
    );
    assert_eq!(
        fixture.lookup("activate-3").await,
        IdempotencyLookup::Missing
    );
    assert!(fixture.deployments.pending().await.unwrap().is_empty());

    fixture.finish().await;
}

#[tokio::test]
async fn a_gateway_without_configuration_receives_the_desired_one() {
    let Some(fixture) = fixture().await else {
        return;
    };
    let original = gateway().await;
    let (_, publication) = fixture.reconciler(&original);
    let prepared = publication
        .prepare(context("prepare-1"), document(1))
        .await
        .unwrap();
    let activated = publication
        .activate(context("activate-1"), prepared.prepare_token().into(), None)
        .await
        .unwrap();

    let replacement = gateway().await;
    assert!(replacement.status().await.unwrap().active_hash().is_none());
    let (reconciler, _) = fixture.reconciler(&replacement);
    assert_eq!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::InSync
    );
    assert_eq!(
        replacement.status().await.unwrap().active_hash(),
        Some(activated.content_hash())
    );
    assert_eq!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::InSync
    );

    fixture.finish().await;
}

#[tokio::test]
async fn unknown_configurations_are_quarantined_and_newer_known_ones_adopted() {
    let Some(fixture) = fixture().await else {
        return;
    };
    let gateway = gateway().await;
    let (reconciler, publication) = fixture.reconciler(&gateway);
    let first = publication
        .prepare(context("prepare-1"), document(1))
        .await
        .unwrap();
    publication
        .activate(context("activate-1"), first.prepare_token().into(), None)
        .await
        .unwrap();

    // Prepared here but activated around this service: adopted.
    let second = publication
        .prepare(context("prepare-2"), document(2))
        .await
        .unwrap();
    gateway
        .activate(
            context("direct-2"),
            second.prepare_token().into(),
            Some(first.content_hash().clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::InSync
    );
    assert_eq!(
        fixture
            .deployments
            .desired()
            .await
            .unwrap()
            .unwrap()
            .revision_id,
        RevisionId::new(2)
    );

    // Never seen by this service: quarantined, and publication suspended.
    let unknown = gateway
        .prepare(context("direct-prepare-3"), document(3))
        .await
        .unwrap();
    gateway
        .activate(
            context("direct-3"),
            unknown.prepare_token().into(),
            Some(second.content_hash().clone()),
        )
        .await
        .unwrap();
    assert!(matches!(
        reconciler.reconcile().await.unwrap(),
        Reconciliation::Quarantined(_)
    ));
    let refused = publication
        .prepare(context("prepare-4"), document(4))
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), ErrorCode::UNAVAILABLE);
    let (_, watch) = Reconciler::new(
        Arc::clone(&gateway),
        Arc::clone(&fixture.receipts),
        fixture.deployments.clone(),
    );
    assert_eq!(
        ReconciliationCheck(watch).check().await.status(),
        HealthStatus::Warn,
        "a reconciler that has not run yet only warns"
    );

    fixture.finish().await;
}
