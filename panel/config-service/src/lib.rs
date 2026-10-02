#![forbid(unsafe_code)]

//! Composition of `config-service`, the write model of gateway
//! configuration and the only caller of the gateway publication protocol.

use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_health::Impact;
use panel_platform::ServiceName;
use panel_postgres::SqlIdentifier;
use panel_service::Environment;
use std::{net::SocketAddr, sync::Arc};

pub const SERVICE: &str = "config-service";
pub const SCHEMA: &str = "config";
pub const GATEWAY_URL_ENV: &str = "PINGORA_PANEL_GATEWAY_URL";
const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:50051";

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9181)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50061)),
    }
}

/// The process; while the gateway is unreachable the service stays
/// readable but suspends publication.
pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let gateway_url = env
        .string(GATEWAY_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.into());
    let gateway = GatewayGrpcClient::connect_lazy(gateway_url, GatewayGrpcClientConfig::default())?;
    Ok(ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )
    .with_check(Arc::new(gateway.health_check()), Impact::Degrading))
}
