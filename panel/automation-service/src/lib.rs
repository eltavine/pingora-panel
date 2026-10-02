#![forbid(unsafe_code)]

//! Composition of `automation-service`, the owner of jobs, schedules and
//! leases, which runs long tasks and calls host operations through
//! `ops-agent`.

use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_platform::ServiceName;
use panel_postgres::SqlIdentifier;
use panel_service::Environment;
use std::net::SocketAddr;

pub const SERVICE: &str = "automation-service";
pub const SCHEMA: &str = "automation";

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9182)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50062)),
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
