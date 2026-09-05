#![forbid(unsafe_code)]

use gateway_grpc::{encode_snapshot, GatewayTransportPolicy};
use gatewayd::{build_gateway_services, gateway_service_name};
use panel_contracts::{
    common::v1 as common,
    gateway::v1::{self as wire, gateway_engine_client::GatewayEngineClient},
};
use panel_domain::RevisionId;
use panel_ir::RuntimeSnapshot;
use std::{fs, net::SocketAddr, path::PathBuf};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Server};
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};
use uuid::Uuid;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("pingora-panel-grpc-{}", Uuid::new_v4())))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RunningGateway {
    address: SocketAddr,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::result::Result<(), tonic::transport::Error>>,
}

impl RunningGateway {
    async fn start(state_directory: PathBuf) -> Self {
        Self::start_with_transport_policy(state_directory, GatewayTransportPolicy::default()).await
    }

    async fn start_with_transport_policy(
        state_directory: PathBuf,
        transport_policy: GatewayTransportPolicy,
    ) -> Self {
        let services = build_gateway_services(state_directory).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let incoming = TcpListenerStream::new(listener);
        let (shutdown, shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            Server::builder()
                .concurrency_limit_per_connection(transport_policy.max_concurrent_requests())
                .timeout(transport_policy.request_timeout())
                .add_service(transport_policy.gateway_server(services.gateway))
                .add_service(services.health)
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_requested.await;
                })
                .await
        });
        Self {
            address,
            shutdown,
            task,
        }
    }

    async fn client(&self) -> GatewayEngineClient<Channel> {
        GatewayEngineClient::connect(format!("http://{}", self.address))
            .await
            .unwrap()
    }

    async fn health_client(&self) -> HealthClient<Channel> {
        let channel = Channel::from_shared(format!("http://{}", self.address))
            .unwrap()
            .connect()
            .await
            .unwrap();
        HealthClient::new(channel)
    }

    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn configured_transport_rejects_oversized_messages_before_dispatch() {
    let state = TemporaryDirectory::new();
    let policy =
        GatewayTransportPolicy::new(1024, 1024, 2, std::time::Duration::from_secs(1)).unwrap();
    let gateway = RunningGateway::start_with_transport_policy(state.0.clone(), policy).await;
    let mut client = gateway.client().await;
    let mut snapshot = encode_snapshot(&RuntimeSnapshot::empty(RevisionId::new(1)));
    snapshot.schema_version = "x".repeat(4096);

    let error = client
        .validate(wire::ValidateRequest {
            context: Some(context("oversized", None)),
            snapshot: Some(snapshot),
        })
        .await
        .unwrap_err();

    assert_eq!(error.code(), tonic::Code::OutOfRange);
    gateway.stop().await;
}

fn context(request_id: &str, idempotency_key: Option<&str>) -> common::RequestContext {
    common::RequestContext {
        request_id: request_id.into(),
        correlation_id: "black-box-flow".into(),
        actor: "black-box-test".into(),
        deadline: String::new(),
        idempotency_key: idempotency_key.unwrap_or_default().into(),
        schema_version: panel_contracts::PROTOCOL_VERSION.into(),
    }
}

#[tokio::test]
async fn generated_client_applies_and_restores_a_snapshot_over_tcp() {
    let state = TemporaryDirectory::new();
    let first = RunningGateway::start(state.0.clone()).await;
    let mut client = first.client().await;
    let mut health_client = first.health_client().await;

    for service in ["", gateway_service_name()] {
        let health = health_client
            .check(HealthCheckRequest {
                service: service.into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(health.status, ServingStatus::Serving as i32);
    }

    let capabilities = client
        .get_capabilities(wire::GetCapabilitiesRequest {
            context: Some(context("capabilities-1", None)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(capabilities.error.is_none());
    assert!(capabilities
        .capabilities
        .iter()
        .any(|capability| capability.name == "activation.cas"));

    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let prepared = client
        .prepare(wire::PrepareRequest {
            context: Some(context("prepare-1", Some("prepare-idempotency-1"))),
            snapshot: Some(encode_snapshot(&snapshot)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(prepared.error.is_none());
    assert!(!prepared.prepare_token.is_empty());

    let activation_request = wire::ActivateRequest {
        context: Some(context("activate-1", Some("activate-idempotency-1"))),
        prepare_token: prepared.prepare_token.clone(),
        expected_active_hash: None,
    };
    let activated = client
        .activate(activation_request.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(activated.error.is_none());
    assert_eq!(activated.revision_id, 1);
    assert_eq!(
        activated.active_hash.as_ref().unwrap().value,
        snapshot.content_hash.as_str()
    );

    let retried = client
        .activate(activation_request)
        .await
        .unwrap()
        .into_inner();
    assert!(retried.error.is_none());
    assert_eq!(retried.active_hash, activated.active_hash);

    let status = client
        .status(wire::StatusRequest {
            context: Some(context("status-1", None)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(status.error.is_none());
    assert_eq!(status.active_revision_id, 1);
    assert_eq!(status.prepared_count, 0);
    let recovery = status.recovery.as_ref().unwrap();
    assert_eq!(recovery.recovery_completed, 1);
    assert_eq!(recovery.degraded_events, 0);
    assert_eq!(recovery.unknown_commit_outcomes, 0);
    let runtime = status.runtime.as_ref().unwrap();
    assert_eq!(runtime.gateway_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(runtime.data_plane_version, "0.8.1");
    assert!(!runtime.adapter_version.is_empty());
    assert_ne!(runtime.started_at_unix_seconds, 0);
    assert_eq!(runtime.worker_count, 1);
    assert_eq!(
        status.health.as_ref().unwrap().state,
        common::health_status::State::Ready as i32
    );
    drop(client);
    first.stop().await;

    let restarted = RunningGateway::start(state.0.clone()).await;
    let mut restarted_client = restarted.client().await;
    let restored = restarted_client
        .status(wire::StatusRequest {
            context: Some(context("status-after-restart", None)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(restored.error.is_none());
    assert_eq!(restored.active_revision_id, 1);
    assert_eq!(restored.active_hash, activated.active_hash);
    let recovery = restored.recovery.as_ref().unwrap();
    assert_eq!(recovery.recovery_completed, 1);
    assert_eq!(recovery.degraded_events, 0);
    assert_eq!(recovery.unknown_commit_outcomes, 0);
    assert_eq!(
        restored.health.unwrap().state,
        common::health_status::State::Ready as i32
    );
    restarted.stop().await;
}

#[tokio::test]
async fn corrupt_lkg_reports_not_serving_while_status_remains_available() {
    let state = TemporaryDirectory::new();
    fs::create_dir_all(&state.0).unwrap();
    fs::write(state.0.join("active.json"), b"not-json").unwrap();
    let gateway = RunningGateway::start(state.0.clone()).await;

    let mut health_client = gateway.health_client().await;
    for service in ["", gateway_service_name()] {
        let health = health_client
            .check(HealthCheckRequest {
                service: service.into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(health.status, ServingStatus::NotServing as i32);
    }

    let mut client = gateway.client().await;
    let status = client
        .status(wire::StatusRequest {
            context: Some(context("corrupt-status", None)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(status.error.is_none());
    let recovery = status.recovery.as_ref().unwrap();
    assert_eq!(recovery.recovery_completed, 1);
    assert_eq!(recovery.degraded_events, 1);
    assert_eq!(recovery.unknown_commit_outcomes, 0);
    assert_eq!(
        status.health.unwrap().state,
        common::health_status::State::NotReady as i32
    );

    let rejected = client
        .prepare(wire::PrepareRequest {
            context: Some(context("corrupt-prepare", Some("corrupt-prepare-1"))),
            snapshot: Some(encode_snapshot(&RuntimeSnapshot::empty(RevisionId::new(1)))),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        rejected.error.unwrap().code,
        panel_errors::ErrorCode::PRECONDITION_FAILED
    );
    gateway.stop().await;
}
