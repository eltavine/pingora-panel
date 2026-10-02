#![forbid(unsafe_code)]

//! Composition of `observability-service`, the owner of alert rules,
//! silences and saved queries, which fronts the metrics and log backends.

use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_platform::ServiceName;
use panel_postgres::SqlIdentifier;
use panel_service::Environment;
use std::net::SocketAddr;

pub const SERVICE: &str = "observability-service";
pub const SCHEMA: &str = "observability";

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9183)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50063)),
    }
}

pub fn process(
    _env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    Ok(ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    ))
}
