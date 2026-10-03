#![forbid(unsafe_code)]

//! Composition of `automation-service`, the owner of jobs, schedules and
//! leases and of the certificate inventory, which runs long tasks and calls
//! host operations through `ops-agent`.
//!
//! Jobs live in the service schema and are run by a worker that leases them;
//! a scheduler enqueues the jobs of due schedules. Certificates keep their
//! private keys sealed with the deployment's master keys and are delivered
//! to the gateway's secret directory. Every change is published as a
//! CloudEvent through the transactional outbox.

mod certificate_api;
mod certificates;
mod delivery;
mod jobs;

pub use certificate_api::CertificateService;
pub use certificates::{Cause, CertificateInventory};
pub use delivery::{Delivery, SecretDirectory};
pub use jobs::PgJobStore;

use panel_contracts::{automation::v1::certificates_server, AUTOMATION_V1};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_jobs::{
    run_scheduler, JobHandler, JobKind, JobStore, ScheduleStore, Worker, WorkerOptions,
};
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_postgres::{EventLog, SchemaMigration, SqlIdentifier};
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_service::Environment;
use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub const SERVICE: &str = "automation-service";
pub const SCHEMA: &str = "automation";
/// Master keys that seal private keys, one base64-encoded 256-bit key per
/// line; the first seals new values. Usually given as `_FILE`.
pub const MASTER_KEYS_ENV: &str = "PINGORA_PANEL_MASTER_KEYS";
/// The gateway's secret directory, where certificates are delivered.
pub const GATEWAY_SECRET_DIR_ENV: &str = "PINGORA_PANEL_GATEWAY_SECRET_DIR";
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(5);
/// How often delivered certificates are compared with the inventory.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

pub const MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration::new(
        10_000,
        "jobs, schedules and maintenance windows",
        include_str!("../migrations/10000_jobs.sql"),
    ),
    SchemaMigration::new(
        10_100,
        "certificate inventory",
        include_str!("../migrations/10100_certificates.sql"),
    ),
];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9182)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50062)),
    }
}

/// The handlers this build runs, by job kind.
pub fn handlers() -> Vec<(JobKind, Arc<dyn JobHandler>)> {
    Vec::new()
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let vault = env
        .secret(MASTER_KEYS_ENV)?
        .map(|keys| EnvelopeVault::from_keys(&keys))
        .transpose()?
        .map(|vault| Arc::new(vault) as Arc<dyn SecretVault>);
    if vault.is_none() {
        tracing::warn!("{MASTER_KEYS_ENV} is not set; certificates cannot be stored");
    }
    let directory = env
        .string(GATEWAY_SECRET_DIR_ENV)?
        .map(SecretDirectory::new);
    let service = ServiceName::new(SERVICE)?;
    let process = ControlPlaneProcess::new(
        service.clone(),
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let store = Arc::new(PgJobStore::new(process.database(), service.clone()));
    let inventory = CertificateInventory::new(
        process.database(),
        EventLog::new(process.database(), service),
        vault,
        directory,
    );
    Ok(process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(AUTOMATION_V1))
        .with_capability(Capability::new("certificates", "1")?)
        .with_peer_access(
            certificates_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_grpc_service(certificates_server::CertificatesServer::new(
            CertificateService::new(inventory.clone()),
        ))
        .on_start(move |running| {
            running.spawn(maintain(
                inventory,
                running.migrated(),
                running.shutdown_token(),
            ));
            let owner = format!("{SERVICE}/{}", running.descriptor().instance_id());
            let worker = handlers().into_iter().fold(
                Worker::new(
                    Arc::clone(&store) as Arc<dyn JobStore>,
                    WorkerOptions::new(owner),
                )
                .with_windows(Arc::clone(&store) as Arc<dyn ScheduleStore>),
                |worker, (kind, handler)| worker.with_handler(kind, handler),
            );
            running.spawn(worker.run(running.shutdown_token()));
            running.spawn(run_scheduler(
                store as Arc<dyn ScheduleStore>,
                SCHEDULER_INTERVAL,
                running.shutdown_token(),
            ));
            Ok(())
        }))
}

/// Once the schema exists, seals keys again with the active master key and
/// keeps the gateway's secret directory in line with the inventory.
async fn maintain(
    inventory: CertificateInventory,
    migrated: impl Future<Output = bool>,
    cancel: CancellationToken,
) {
    tokio::select! {
        () = cancel.cancelled() => return,
        ready = migrated => if !ready { return },
    }
    match inventory.reseal().await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "certificate keys sealed with the active master key"),
        Err(error) => tracing::warn!(error_code = %error.code, "certificate keys not resealed"),
    }
    loop {
        match inventory.reconcile().await {
            Ok(0) => {}
            Ok(files) => tracing::info!(files, "certificate files delivered"),
            Err(error) => tracing::warn!(error_code = %error.code, "certificates not delivered"),
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(RECONCILE_INTERVAL) => {}
        }
    }
}
