#![forbid(unsafe_code)]

//! Composition of `observability-service`, the owner of alert rules,
//! silences and saved queries, which fronts the metrics and log backends.

mod promql;
mod traffic;

pub use promql::Scope;
pub use traffic::TrafficService;

use panel_contracts::{observability::v1::traffic_server, OBSERVABILITY_V1};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::{PanelError, Result};
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_postgres::SqlIdentifier;
use panel_service::Environment;
use std::{net::SocketAddr, time::Duration};

pub const SERVICE: &str = "observability-service";
pub const SCHEMA: &str = "observability";

/// Where Prometheus, which keeps the gateway's metrics, answers queries.
pub const PROMETHEUS_URL_ENV: &str = "PINGORA_PANEL_PROMETHEUS_URL";
const DEFAULT_PROMETHEUS_URL: &str = "http://127.0.0.1:9090";
const PROMETHEUS_TIMEOUT: Duration = Duration::from_secs(10);

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9183)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50063)),
    }
}

/// A client of the Prometheus HTTP API at `url`.
pub fn prometheus(url: &str) -> Result<prometheus_http_query::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .timeout(PROMETHEUS_TIMEOUT)
        .build()
        .map_err(|error| PanelError::internal(format!("cannot build an HTTP client: {error}")))?;
    prometheus_http_query::Client::from(client, url).map_err(|error| {
        PanelError::invalid_argument(format!("invalid {PROMETHEUS_URL_ENV}: {error}"))
    })
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let url = env
        .string(PROMETHEUS_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_PROMETHEUS_URL.to_owned());
    let traffic = TrafficService::new(prometheus(&url)?);
    Ok(ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?
    .with_protocol(protocol_range(OBSERVABILITY_V1))
    .with_capability(Capability::new("observability.traffic", "1")?)
    .with_peer_access(
        traffic_server::SERVICE_NAME,
        [ServiceName::new("panel-api")?],
    )
    .with_grpc_service(traffic_server::TrafficServer::new(traffic)))
}
