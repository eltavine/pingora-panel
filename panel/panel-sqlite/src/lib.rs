#![forbid(unsafe_code)]

//! SQLite adapter for module-owned persistence (ADR 0032).
//!
//! Every module of the control plane owns one database file, readable by
//! the control plane's user alone, opened in write-ahead-log mode with full
//! synchronization and foreign keys. No transaction spans files, and
//! transactions that write take the file's lock when they begin.

mod database;
mod error;
mod event_log;
mod health;
mod inbox;
mod outbox;
mod time;

pub use database::{SchemaMigration, ServiceDatabase, ServiceDatabaseConfig};
pub use error::storage_error;
pub use event_log::EventLog;
pub use health::SqliteHealthCheck;
pub use inbox::SqliteProcessedEventStore;
pub use outbox::{OutboxBacklog, SqliteOutbox, SqliteOutboxWakeup};

#[cfg(feature = "test-support")]
pub mod testing;
