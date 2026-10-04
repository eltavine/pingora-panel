#![forbid(unsafe_code)]

//! Transport and storage neutral application ports for the control plane.
//!
//! This crate deliberately does not depend on `panel-engine`, Pingora, Tonic,
//! Axum, SQLx, or a GUI. Adapters own those dependencies and translate at the
//! boundary. Internal modules are private so their organization can evolve
//! without changing the public application contract.

/// Declares the operations of a port's contract as an enum, each variant
/// serialized under the operation's name, which audit records and receipts
/// know it by, with its parameters beside it. The caller depends on `serde`.
#[macro_export]
macro_rules! operations {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $operation:literal => $variant:ident
                $({ $($(#[$field_meta:meta])* $field:ident: $type:ty),* $(,)? })?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "operation", content = "parameters", deny_unknown_fields)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                #[serde(rename = $operation)]
                $variant $({ $($(#[$field_meta])* $field: $type),* })?,
            )*
        }

        impl $name {
            /// The operation's name.
            pub fn operation(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $operation,)*
                }
            }
        }
    };
}

mod alerts;
mod audit;
mod containers;
mod context;
mod gateway;
mod host;
mod host_agent;
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
pub use containers::{
    ContainerAction, ContainerChange, ContainerDetail, ContainerEngine, ContainerFilter,
    ContainerList, ContainerLogLine, ContainerLogQuery, ContainerLogStart, ContainerLogStream,
    ContainerLogTail, ContainerLogs, ContainerMount, ContainerNetwork, ContainerState,
    ContainerSummary, ContainersPort, EngineInfo, EngineVersion, NoContainers, PortMapping,
    RecordedContainers,
};
pub use context::{
    Actor, CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope, SiteAccess,
    SiteScope, TraceContext,
};
pub use gateway::{
    AbortOutcome, ActivatedDeployment, ConfigCompiler, ConfigDocument, DeploymentOutcome,
    GatewayPort, GatewayService, GatewayStatus, GatewayUseCases, PreparedDeployment,
};
pub use host::{HostFilesystem, HostNetworkDevice, HostPort, HostSummary};
pub use host_agent::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, DirectoriesReport,
    DirectoryKind, DirectoryUsage, GatewayContainer, GatewayServiceAction, GatewayServiceStatus,
    HostAgentPort, ListenersReport, ListeningProcess, NoHostAgent, PortListener, RecordedHostAgent,
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
