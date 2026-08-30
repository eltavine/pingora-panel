//! `gatewayd` composition factory.
//!
//! The binary owns executor startup and OS signal selection. This library owns
//! configuration parsing and concrete adapter wiring so production and black-box
//! tests use the same dependency graph.

mod bind_policy;
mod config;
mod runtime_info;
mod shutdown;

pub use bind_policy::{LoopbackOnlyManagementBindPolicy, ManagementBindPolicy};
pub use config::{
    GatewayWorkerCount, GatewaydConfig, DRAIN_TIMEOUT_MILLIS_ENV, GATEWAY_ADDRESS_ENV,
    MAX_GATEWAY_WORKERS, STATE_DIRECTORY_ENV, WORKER_COUNT_ENV,
};
pub use runtime_info::ProcessRuntimeInfo;
pub use shutdown::{ReadinessGate, ShutdownCoordinator, ShutdownPolicy, TonicHealthReadinessGate};

use gateway_grpc::GatewayGrpcService;
use gateway_pingora::PingoraGatewayAdapter;
use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngineServer;
use panel_engine::{GatewayEngine, GatewayRuntimeInfoProvider};
use panel_errors::Result;
use panel_gateway_runtime::DurableGatewayEngine;
use snapshot_store_fs::FileSnapshotStore;
use std::{future::Future, num::NonZeroU32, path::PathBuf, sync::Arc};
use tonic::server::NamedService;
use tonic::transport::Server;
use tonic_health::{
    pb::health_server::HealthServer,
    server::{HealthReporter, HealthService},
    ServingStatus,
};

pub type GatewaydEngine = DurableGatewayEngine<PingoraGatewayAdapter, FileSnapshotStore>;
pub type GatewaydTransport = GatewayGrpcService<GatewaydEngine>;
pub type GatewaydHealth = HealthServer<HealthService>;

pub struct GatewaydServices {
    pub gateway: GatewaydTransport,
    pub health: GatewaydHealth,
    pub health_reporter: HealthReporter,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewaydError {
    #[error("gateway configuration or composition failed: {0}")]
    Panel(#[from] panel_errors::PanelError),
    #[error("gateway transport failed: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gateway executor failed: {0}")]
    Executor(#[from] std::io::Error),
}

pub fn gateway_service_name() -> &'static str {
    <GatewayEngineServer<GatewaydTransport> as NamedService>::NAME
}

pub async fn build_gateway_services(
    state_directory: impl Into<PathBuf>,
) -> Result<GatewaydServices> {
    let runtime_info = Arc::new(ProcessRuntimeInfo::new(
        env!("CARGO_PKG_VERSION"),
        NonZeroU32::MIN,
    ));
    build_gateway_services_with_runtime_info(state_directory, runtime_info).await
}

pub async fn build_gateway_services_with_runtime_info(
    state_directory: impl Into<PathBuf>,
    runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
) -> Result<GatewaydServices> {
    let adapter = Arc::new(PingoraGatewayAdapter::new());
    let store = Arc::new(FileSnapshotStore::new(state_directory));
    let engine = Arc::new(DurableGatewayEngine::restore(adapter, store).await?);
    let serving_status = match engine.status().await {
        Ok(status) if status.ready => ServingStatus::Serving,
        Ok(_) | Err(_) => ServingStatus::NotServing,
    };

    let health_reporter = HealthReporter::new();
    health_reporter.set_service_status("", serving_status).await;
    health_reporter
        .set_service_status(gateway_service_name(), serving_status)
        .await;
    let health = HealthServer::new(HealthService::from_health_reporter(health_reporter.clone()));

    Ok(GatewaydServices {
        gateway: GatewayGrpcService::with_runtime_info(engine, runtime_info),
        health,
        health_reporter,
    })
}

pub async fn serve_gatewayd(
    config: GatewaydConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::result::Result<(), GatewaydError> {
    let runtime_info = Arc::new(ProcessRuntimeInfo::new(
        env!("CARGO_PKG_VERSION"),
        config.worker_count().as_non_zero(),
    ));
    let services = build_gateway_services_with_runtime_info(
        config.state_directory().to_path_buf(),
        runtime_info,
    )
    .await?;
    let readiness = Arc::new(TonicHealthReadinessGate::new(
        services.health_reporter.clone(),
        ["", gateway_service_name()],
    ));
    let shutdown_coordinator = ShutdownCoordinator::new(readiness, config.shutdown_policy());

    Server::builder()
        .add_service(GatewayEngineServer::new(services.gateway))
        .add_service(services.health)
        .serve_with_shutdown(config.listen_address(), shutdown_coordinator.run(shutdown))
        .await?;
    Ok(())
}

pub async fn build_gateway_transport(
    state_directory: impl Into<PathBuf>,
) -> Result<GatewaydTransport> {
    Ok(build_gateway_services(state_directory).await?.gateway)
}
