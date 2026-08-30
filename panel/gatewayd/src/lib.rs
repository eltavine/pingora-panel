//! `gatewayd` composition factory.
//!
//! Process concerns remain in `main`; this module only wires concrete adapters to
//! the durable application runtime so production and black-box tests use the same
//! dependency graph.

use gateway_grpc::GatewayGrpcService;
use gateway_pingora::PingoraGatewayAdapter;
use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngineServer;
use panel_engine::GatewayEngine;
use panel_errors::Result;
use panel_gateway_runtime::DurableGatewayEngine;
use snapshot_store_fs::FileSnapshotStore;
use std::{path::PathBuf, sync::Arc};
use tonic::server::NamedService;
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

pub fn gateway_service_name() -> &'static str {
    <GatewayEngineServer<GatewaydTransport> as NamedService>::NAME
}

pub async fn build_gateway_services(
    state_directory: impl Into<PathBuf>,
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
        gateway: GatewayGrpcService::new(engine),
        health,
        health_reporter,
    })
}

pub async fn build_gateway_transport(
    state_directory: impl Into<PathBuf>,
) -> Result<GatewaydTransport> {
    Ok(build_gateway_services(state_directory).await?.gateway)
}
