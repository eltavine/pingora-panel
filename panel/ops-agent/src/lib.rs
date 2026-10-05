#![forbid(unsafe_code)]
#![cfg(unix)]

//! The agent on the host for what no container may do (ADR 0028, ADR 0030).
//! It serves gRPC over mutual TLS on a Unix domain socket to `panel-api`
//! only, one service per capability, each bounded by this configuration.

mod agent;
mod compose;
pub mod config;
mod container_logs;
mod container_stats;
mod containers;
mod directories;
mod engine_disk;
mod engine_prune;
mod engine_resources;
#[cfg(test)]
mod fake_engine;
mod gateway_service;
mod image_pull;
mod images;
mod listeners;
mod socket;

pub use config::AgentConfig;

use panel_context::ServiceName;
use panel_contracts::ops::v1::{
    agent_server::{self, AgentServer},
    compose_projects_server::{self, ComposeProjectsServer},
    containers_server::{self, ContainersServer},
    directories_server::{self, DirectoriesServer},
    engine_resources_server::{self, EngineResourcesServer},
    gateway_service_server::{self, GatewayServiceServer},
    images_server::{self, ImagesServer},
    CapabilityState,
};
use panel_errors::{PanelError, Result};
use panel_pki::{CredentialFiles, WorkloadIdentity};
use panel_tls::{PeerPolicy, TlsCredentials};
use std::{future::Future, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;

/// The agent's workload identity.
pub const SERVICE: &str = "ops-agent";

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CREDENTIAL_RELOAD_INTERVAL: Duration = Duration::from_secs(60);

/// Serves until `shutdown` resolves, then removes the socket.
pub async fn serve(config: AgentConfig, shutdown: impl Future<Output = ()> + Send) -> Result<()> {
    let credentials = TlsCredentials::load(
        CredentialFiles::new(&config.credentials),
        WorkloadIdentity::new(ServiceName::new(SERVICE)?, config.trust_domain.clone()),
    )?;
    let listener = socket::bind(&config.socket, config.socket_group)?;
    let panel_api = [ServiceName::new("panel-api")?];
    let mut policy = PeerPolicy::new(config.trust_domain.clone())
        .allow(agent_server::SERVICE_NAME, panel_api.clone());

    let listeners = listeners::capability(config.listeners);
    let serves_listeners = listeners.state() == CapabilityState::Available;
    let agent = agent::AgentService::new(vec![
        agent::directories(&config),
        listeners,
        containers::capability(&config.engines),
        gateway_service::capability(&config.engines),
    ]);
    let (container_service, image_service, resource_service, compose_service, gateway_service) =
        if config.engines.is_empty() {
            (None, None, None, None, None)
        } else {
            policy = policy
                .allow(containers_server::SERVICE_NAME, panel_api.clone())
                .allow(images_server::SERVICE_NAME, panel_api.clone())
                .allow(engine_resources_server::SERVICE_NAME, panel_api.clone())
                .allow(compose_projects_server::SERVICE_NAME, panel_api.clone())
                .allow(gateway_service_server::SERVICE_NAME, panel_api.clone());
            let engines = Arc::new(containers::Engines::load(
                config.engines.clone(),
                config.state.clone(),
            ));
            (
                Some(ContainersServer::new(containers::ContainerService::new(
                    Arc::clone(&engines),
                    config.installation_project.clone(),
                ))),
                Some(ImagesServer::new(images::ImageService::new(
                    Arc::clone(&engines),
                    config.installation_project.clone(),
                ))),
                Some(EngineResourcesServer::new(
                    engine_resources::ResourceService::new(
                        Arc::clone(&engines),
                        config.installation_project.clone(),
                    ),
                )),
                Some(ComposeProjectsServer::new(compose::ComposeService::new(
                    Arc::clone(&engines),
                    config.installation_project.clone(),
                ))),
                Some(GatewayServiceServer::new(
                    gateway_service::GatewayService::new(
                        engines,
                        config.installation_project.clone(),
                        config.gateway_service.clone(),
                    ),
                )),
            )
        };
    let directories = if config.directories.is_empty() {
        None
    } else {
        policy = policy.allow(directories_server::SERVICE_NAME, panel_api.clone());
        Some(DirectoriesServer::new(directories::DirectoryService::new(
            config.directories.clone(),
        )))
    };
    #[cfg(target_os = "linux")]
    let listener_service = if serves_listeners {
        policy = policy.allow(
            panel_contracts::ops::v1::listeners_server::SERVICE_NAME,
            panel_api.clone(),
        );
        Some(
            panel_contracts::ops::v1::listeners_server::ListenersServer::new(
                listeners::ListenerService,
            ),
        )
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let _ = serves_listeners;
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    tracing::info!(
        event = "agent_started",
        socket = %config.socket.display(),
        capabilities = %agent.description().version.as_ref().map(|version| version.capability_set.as_str()).unwrap_or_default(),
    );

    let reload = CancellationToken::new();
    let watcher =
        tokio::spawn(Arc::clone(&credentials).watch(CREDENTIAL_RELOAD_INTERVAL, reload.clone()));
    let router = Server::builder()
        .layer(policy)
        .add_service(health)
        .add_service(AgentServer::new(agent))
        .add_optional_service(directories)
        .add_optional_service(container_service)
        .add_optional_service(image_service)
        .add_optional_service(resource_service)
        .add_optional_service(compose_service)
        .add_optional_service(gateway_service);
    #[cfg(target_os = "linux")]
    let router = router.add_optional_service(listener_service);
    let served = router
        .serve_with_incoming_shutdown(
            panel_tls::incoming_unix(
                listener,
                config.peer_users.clone(),
                credentials,
                HANDSHAKE_TIMEOUT,
            ),
            shutdown,
        )
        .await;
    reload.cancel();
    let _ = watcher.await;
    let _ = std::fs::remove_file(&config.socket);
    served.map_err(|error| PanelError::unavailable(format!("the agent stopped serving: {error}")))
}
