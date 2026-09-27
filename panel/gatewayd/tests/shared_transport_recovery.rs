#![forbid(unsafe_code)]

#[path = "support/management.rs"]
mod management;

use gateway_grpc_client::GatewayGrpcClient;
use gatewayd::{
    build_gateway_runtime_with_options, GatewaydServiceOptions, GatewaydServices,
    ProcessRuntimeInfo,
};
use management::{json, problem, ManagementServer};
use panel_application::GatewayPort;
use panel_domain::RevisionId;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use serde_json::json as value;
use std::{net::SocketAddr, num::NonZeroU32, sync::Arc};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

fn runtime_info() -> Arc<ProcessRuntimeInfo> {
    Arc::new(ProcessRuntimeInfo::new("test", NonZeroU32::new(1).unwrap()))
}

struct GrpcServer {
    address: SocketAddr,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::result::Result<(), tonic::transport::Error>>,
}

impl GrpcServer {
    async fn start(services: &GatewaydServices) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, requested) = oneshot::channel();
        let gateway = services.gateway();
        let health = services.health();
        let task = tokio::spawn(async move {
            let policy = gateway.transport_policy();
            Server::builder()
                .concurrency_limit_per_connection(policy.max_concurrent_requests())
                .timeout(policy.request_timeout())
                .add_service(policy.gateway_server(gateway))
                .add_service(health)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = requested.await;
                })
                .await
        });
        Self {
            address,
            shutdown,
            task,
        }
    }

    async fn client(&self) -> GatewayGrpcClient {
        GatewayGrpcClient::connect(format!("http://{}", self.address))
            .await
            .unwrap()
    }

    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn http_publication_is_visible_over_grpc_and_after_restart() {
    let state = tempfile::tempdir().unwrap();
    let runtime = build_gateway_runtime_with_options(
        state.path(),
        runtime_info(),
        GatewaydServiceOptions::default(),
    )
    .await
    .unwrap();
    let management = ManagementServer::start(runtime.engine(), 8192).await;
    let grpc_server = GrpcServer::start(&runtime.services).await;
    let grpc = grpc_server.client().await;

    let document = value!({
        "schema_version": IR_SCHEMA_VERSION,
        "snapshot": RuntimeSnapshot::empty(RevisionId::new(1)),
    });
    let prepared = json(
        management
            .mutation("/api/v1/gateway/prepare", &document, "prepare-shared")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let activated = json(
        management
            .mutation(
                "/api/v1/gateway/activate",
                &value!({"prepare_token": prepared["prepare_token"]}),
                "activate-shared",
            )
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let active_hash = activated["content_hash"].as_str().unwrap().to_owned();
    let http_status = json(
        management
            .get("/api/v1/gateway/status")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(http_status["active_hash"], active_hash);
    assert_eq!(
        grpc.status().await.unwrap().active_hash().unwrap().as_str(),
        active_hash
    );
    problem(
        management
            .mutation(
                "/api/v1/gateway/abort",
                &value!({"prepare_token": "missing"}),
                "abort-missing",
            )
            .send()
            .await
            .unwrap(),
        404,
        "NOT_FOUND",
    )
    .await;

    management.stop().await;
    drop(grpc);
    grpc_server.stop().await;
    runtime.background_tasks.shutdown_and_join().await.unwrap();
    drop(runtime);

    let restored = build_gateway_runtime_with_options(
        state.path(),
        runtime_info(),
        GatewaydServiceOptions::default(),
    )
    .await
    .unwrap();
    let management = ManagementServer::start(restored.engine(), 8192).await;
    let grpc_server = GrpcServer::start(&restored.services).await;
    let grpc = grpc_server.client().await;
    assert_eq!(
        restored
            .engine()
            .status()
            .await
            .unwrap()
            .active_hash
            .unwrap()
            .as_str(),
        active_hash
    );
    assert_eq!(
        json(
            management
                .get("/api/v1/gateway/status")
                .send()
                .await
                .unwrap(),
            200,
        )
        .await["active_hash"],
        active_hash
    );
    assert_eq!(
        grpc.status().await.unwrap().active_hash().unwrap().as_str(),
        active_hash
    );
    management.stop().await;
    drop(grpc);
    grpc_server.stop().await;
    restored.background_tasks.shutdown_and_join().await.unwrap();
}
