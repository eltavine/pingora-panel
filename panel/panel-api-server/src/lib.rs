#![forbid(unsafe_code)]

//! Composition of `panel-api`, the single public management entry point.
//!
//! The process serves the REST API and the web console on its public
//! listener. Publication is delegated to `config-service`; the service
//! directory is read from the broker. While a dependency the API needs for
//! changes is down the process runs degraded: reads continue and changes
//! are refused with a retryable 503.

mod console;
mod directory;

use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use panel_api::{router_with_config, ApiConfig, ApiState};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::{PanelError, Result};
use panel_health::Impact;
use panel_platform::ServiceName;
use panel_postgres::SqlIdentifier;
use panel_service::{require_loopback, Environment};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

pub const SERVICE: &str = "panel-api";
pub const SCHEMA: &str = "identity";
/// The public listener; loopback-only until the API authenticates callers.
pub const HTTP_ADDRESS_ENV: &str = "PINGORA_PANEL_HTTP_ADDR";
pub const CONFIG_URL_ENV: &str = "PINGORA_PANEL_CONFIG_URL";
/// The gateway's runtime API, for data plane operations and upstream health.
pub const GATEWAY_URL_ENV: &str = "PINGORA_PANEL_GATEWAY_URL";
/// Directory holding the built web console; the API is served without it.
pub const WEB_ROOT_ENV: &str = "PINGORA_PANEL_WEB_ROOT";

const DEFAULT_HTTP_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);
const DEFAULT_CONFIG_URL: &str = "http://127.0.0.1:50061";
const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:50051";
const DEFAULT_WEB_ROOT: &str = "/usr/share/pingora-panel/web";

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9180)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50060)),
    }
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let http_address = require_loopback(
        HTTP_ADDRESS_ENV,
        env.socket_addr(HTTP_ADDRESS_ENV, DEFAULT_HTTP_ADDRESS)?,
    )?;
    let config_url = env
        .string(CONFIG_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_CONFIG_URL.into());
    let gateway_url = env
        .string(GATEWAY_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.into());
    let web_root = PathBuf::from(
        env.string(WEB_ROOT_ENV)?
            .unwrap_or_else(|| DEFAULT_WEB_ROOT.into()),
    );
    // Bound now so a taken port fails the start before anything else runs.
    let listener = std::net::TcpListener::bind(http_address).map_err(|error| {
        PanelError::precondition_failed(format!(
            "cannot bind the public listener on {http_address}: {error}"
        ))
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        PanelError::internal(format!("cannot configure the public listener: {error}"))
    })?;
    let process = ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let config = match process.peer_channel(&config_url, ServiceName::new("config-service")?)? {
        Some(channel) => {
            ConfigPublicationClient::from_channel(channel, ConfigClientConfig::default())
        }
        None => ConfigPublicationClient::connect_lazy(config_url, ConfigClientConfig::default())?,
    };
    let gateway = match process.peer_channel(&gateway_url, ServiceName::new("gatewayd")?)? {
        Some(channel) => GatewayGrpcClient::from_channel_with_config(
            channel,
            GatewayGrpcClientConfig::default(),
        )?,
        None => GatewayGrpcClient::connect_lazy(gateway_url, GatewayGrpcClientConfig::default())?,
    };
    let config_health = config.health_check();
    Ok(process
        .with_database_impact(Impact::Degrading)
        .with_check(Arc::new(config_health), Impact::Degrading)
        .on_start(move |running| {
            let config = Arc::new(config);
            let api = router_with_config(
                ApiState::new(Arc::clone(&config))
                    .with_configuration(config)
                    .with_runtime(Arc::new(gateway))
                    .with_health(running.health())
                    .with_directory(Arc::new(directory::RegistryDirectory::new(
                        running.jetstream().clone(),
                        Arc::clone(running.jetstream_settings()),
                    ))),
                ApiConfig::default(),
            );
            let app = console::with_console(api, &web_root)?;
            let listener = tokio::net::TcpListener::from_std(listener).map_err(|error| {
                PanelError::internal(format!("cannot serve the public listener: {error}"))
            })?;
            let address = listener.local_addr().ok();
            let shutdown = running.shutdown_token();
            running.spawn(async move {
                if let Err(error) = axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await
                {
                    tracing::error!(%error, "public listener failed");
                }
            });
            tracing::info!(address = ?address, "public API listening");
            Ok(())
        }))
}
