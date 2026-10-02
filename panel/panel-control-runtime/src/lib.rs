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

mod main_loop;
mod process;
mod retry;
mod settings;
mod tasks;

pub use main_loop::{service_main, HEALTHCHECK_ARGUMENT};
pub use process::{ControlPlaneProcess, RunningProcess};
pub use settings::{
    DefaultAddresses, ProcessSettings, TlsSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV,
    GRPC_ADDRESS_ENV, HEALTH_INTERVAL_MS_ENV, NATS_URL_ENV, OPS_ADDRESS_ENV, TLS_DIR_ENV,
    TRUST_DOMAIN_ENV,
};
