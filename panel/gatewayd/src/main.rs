use gatewayd::build_gateway_services;
use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngineServer;
use std::{error::Error, net::SocketAddr, path::PathBuf};
use tonic::transport::Server;

const DEFAULT_LISTEN_ADDRESS: &str = "127.0.0.1:50051";
const DEFAULT_STATE_DIRECTORY: &str = "/var/lib/pingora-panel/gateway";

struct GatewaydConfig {
    listen_address: SocketAddr,
    state_directory: PathBuf,
}

impl GatewaydConfig {
    fn from_environment() -> Result<Self, Box<dyn Error>> {
        let listen_address = std::env::var("PINGORA_PANEL_GATEWAY_ADDR")
            .unwrap_or_else(|_| DEFAULT_LISTEN_ADDRESS.into())
            .parse()?;
        let state_directory = std::env::var_os("PINGORA_PANEL_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY));
        Ok(Self {
            listen_address,
            state_directory,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = GatewaydConfig::from_environment()?;
    let services = build_gateway_services(config.state_directory).await?;

    Server::builder()
        .add_service(GatewayEngineServer::new(services.gateway))
        .add_service(services.health)
        .serve_with_shutdown(config.listen_address, shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
