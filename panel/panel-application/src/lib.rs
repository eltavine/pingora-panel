#![forbid(unsafe_code)]

//! Transport and storage neutral application ports for the control plane.
//!
//! This crate deliberately does not depend on `panel-engine`, Pingora, Tonic,
//! Axum, SQLx, or a GUI. Adapters own those dependencies and translate at the
//! boundary. Internal modules are private so their organization can evolve
//! without changing the public application contract.

mod context;
mod gateway;
mod persistence;

pub use context::{CommandContext, IdempotencyKey, RequestDeadline, RequestId};
pub use gateway::{
    ActivatedDeployment, ConfigCompiler, ConfigDocument, DeploymentOutcome, GatewayPort,
    GatewayService, GatewayUseCases, IdempotentGatewayUseCases, PreparedDeployment,
};
pub use panel_domain::ContentHash;
pub use persistence::{
    AuditEventStore, AuditFact, IdempotencyClaim, IdempotencyRecord, IdempotencyRepository,
    RevisionRepository,
};
