#![forbid(unsafe_code)]

//! PostgreSQL adapter for service-owned persistence.
//!
//! Every service owns exactly one schema and connects as a role that owns
//! that schema and nothing else. Bootstrap applies PostgreSQL's secure schema
//! usage pattern: no role other than the owner may create objects in a
//! schema, `PUBLIC` loses default privileges, and each role's `search_path`
//! names only its own schema, so unqualified platform SQL resolves to the
//! caller's schema and cannot reach another service's tables.

mod bootstrap;
mod database;
mod error;
mod identifier;
mod inbox;
mod outbox;
mod scram;

pub use bootstrap::{DatabaseBootstrap, ServiceRole};
pub use database::{SchemaMigration, ServiceDatabase, ServiceDatabaseConfig};
pub use error::storage_error;
pub use identifier::SqlIdentifier;
pub use inbox::PgProcessedEventStore;
pub use outbox::{OutboxBacklog, PgOutbox, PgOutboxWakeup, PgRelayLeadership};
pub use scram::{RoleSecret, ScramVerifier};

#[cfg(feature = "test-support")]
pub mod testing;
