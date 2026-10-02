#![forbid(unsafe_code)]

//! Composition of control-plane service processes.
//!
//! A service declares its name, schema, migrations, dependencies and gRPC
//! services; the runtime binds its listeners, then brings up everything that
//! depends on PostgreSQL or NATS in the background with retries. Until the
//! schema is migrated the service reports itself unavailable; afterwards
//! each dependency affects readiness according to its declared impact.
//! Every process exposes liveness and readiness, standard gRPC health and
//! its service description, registers itself in the service directory, and
//! relays its transactional outbox while it leads.

mod process;
mod retry;
mod settings;
mod tasks;

pub use process::{ControlPlaneProcess, RunningProcess};
pub use settings::{
    DefaultAddresses, ProcessSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, GRPC_ADDRESS_ENV,
    HEALTH_INTERVAL_MS_ENV, NATS_URL_ENV, OPS_ADDRESS_ENV,
};
