#![forbid(unsafe_code)]

//! Composition of `automation-service`, the owner of jobs, schedules and
//! leases, which runs long tasks and calls host operations through
//! `ops-agent`.
//!
//! Jobs live in the service schema and are run by a worker that leases them;
//! a scheduler enqueues the jobs of due schedules. Every change of a job is
//! published as a CloudEvent through the transactional outbox.

mod jobs;

pub use jobs::PgJobStore;

use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_jobs::{
    run_scheduler, JobHandler, JobKind, JobStore, ScheduleStore, Worker, WorkerOptions,
};
use panel_platform::ServiceName;
use panel_postgres::{SchemaMigration, SqlIdentifier};
use panel_service::Environment;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub const SERVICE: &str = "automation-service";
pub const SCHEMA: &str = "automation";
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(5);

pub const MIGRATIONS: &[SchemaMigration] = &[SchemaMigration::new(
    10_000,
    "jobs, schedules and maintenance windows",
    include_str!("../migrations/10000_jobs.sql"),
)];

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
    _env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let service = ServiceName::new(SERVICE)?;
    let process = ControlPlaneProcess::new(
        service.clone(),
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let store = Arc::new(PgJobStore::new(process.database(), service));
    Ok(process
        .with_migrations(MIGRATIONS)
        .on_start(move |running| {
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
