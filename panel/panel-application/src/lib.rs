#![forbid(unsafe_code)]

//! Transport and storage neutral application ports for the control plane.
//!
//! This crate deliberately does not depend on `panel-engine`, Pingora, Tonic,
//! Axum, SQLx, or a GUI. Adapters own those dependencies and translate at the
//! boundary. Internal modules are private so their organization can evolve
//! without changing the public application contract.

mod configuration;
mod context;
mod gateway;
mod idempotency;
mod persistence;
mod runtime;

pub use configuration::{
    ApplyOutcome, ConfigurationChange, ConfigurationOutput, ConfigurationPort, ConfigurationRead,
    DraftInfo,
};
pub use context::{
    Actor, CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope, TraceContext,
};
pub use gateway::{
    AbortOutcome, ActivatedDeployment, ConfigCompiler, ConfigDocument, DeploymentOutcome,
    GatewayPort, GatewayService, GatewayStatus, GatewayUseCases, PreparedDeployment,
};
pub use idempotency::IdempotentGatewayUseCases;
pub use panel_domain::ContentHash;
pub use persistence::{
    AuditEventStore, AuditFact, IdempotencyClaim, IdempotencyLookup, IdempotencyRecord,
    IdempotencyRepository, RevisionRepository,
};
pub use runtime::{
    DataPlaneListener, DataPlaneState, EndpointHealth, GatewayRuntimePort, UpstreamHealth,
    UpstreamHealthReport,
};
