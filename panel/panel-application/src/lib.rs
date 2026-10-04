#![forbid(unsafe_code)]

//! Transport and storage neutral application ports for the control plane.
//!
//! This crate deliberately does not depend on `panel-engine`, Pingora, Tonic,
//! Axum, SQLx, or a GUI. Adapters own those dependencies and translate at the
//! boundary. Internal modules are private so their organization can evolve
//! without changing the public application contract.

mod alerts;
mod audit;
mod certificates;
mod configuration;
mod context;
mod gateway;
mod idempotency;
mod logs;
mod operations;
mod persistence;
mod runtime;
mod tls_probe;
mod traffic;

pub use alerts::{
    AlertChannel, AlertChannelKind, AlertChannelSecret, AlertComparison, AlertMeasure,
    AlertNotification, AlertNotificationKind, AlertNotificationQuery, AlertNotificationState,
    AlertRule, AlertRuleSpec, AlertSeverity, AlertState, AlertTest, AlertsPort, NewAlertChannel,
};
pub use audit::{AuditFilter, AuditPage, AuditPort, AuditRecord, AuditVerification};
pub use certificates::{CertificateChange, CertificateOutput, CertificatePort, CertificateRead};
pub use configuration::{
    ApplyOutcome, ApplyRequest, ApprovalBypass, ConfigurationChange, ConfigurationOutput,
    ConfigurationPort, ConfigurationRead, DraftInfo,
};
pub use context::{
    Actor, CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope, SiteAccess,
    SiteScope, TraceContext,
};
pub use gateway::{
    AbortOutcome, ActivatedDeployment, ConfigCompiler, ConfigDocument, DeploymentOutcome,
    GatewayPort, GatewayService, GatewayStatus, GatewayUseCases, PreparedDeployment,
};
pub use idempotency::IdempotentGatewayUseCases;
pub use logs::{
    LogBatch, LogDeletion, LogDeletionState, LogFilter, LogKind, LogPage, LogRecord, LogSearch,
    LogTail, LogsPort, RecordedLogs,
};
pub use operations::{Operation, OperationLog, RecordedRuntime};
pub use panel_domain::{ContentHash, RouteId, SiteId, UpstreamPoolId};
pub use persistence::{
    AuditEventStore, AuditFact, IdempotencyClaim, IdempotencyLookup, IdempotencyRecord,
    IdempotencyRepository, RevisionRepository,
};
pub use runtime::{
    DataPlaneListener, DataPlaneState, EndpointHealth, EscapingLink, FileChecks,
    GatewayRuntimePort, PrivateKeyCheck, StaticRootCheck, UpstreamHealth, UpstreamHealthReport,
};
pub use tls_probe::{TlsProbe, TlsProbeReport, TlsProbeTarget};
pub use traffic::{
    DomainTraffic, Latency, RouteTraffic, StatusClasses, TrafficPoint, TrafficPort, TrafficQuery,
    TrafficSummary, UpstreamFailure, UpstreamTraffic,
};
