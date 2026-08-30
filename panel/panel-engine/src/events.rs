use panel_domain::RevisionId;
use panel_errors::ErrorCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayOperation {
    RestoreActive,
    RestorePrepared,
    SavePrepared,
    CommitActivation,
    DeletePrepared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayRequestOperation {
    GetCapabilities,
    Validate,
    Prepare,
    Activate,
    Abort,
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayRequestMetadata {
    pub request_id: String,
    pub correlation_id: String,
    pub actor: String,
}

impl GatewayRequestMetadata {
    pub fn new(
        request_id: impl Into<String>,
        correlation_id: impl Into<String>,
        actor: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            correlation_id: correlation_id.into(),
            actor: actor.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayRequestOutcome {
    Succeeded,
    Rejected { error_code: ErrorCode },
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayEvent {
    RecoveryCompleted {
        ready: bool,
        active_revision_id: Option<RevisionId>,
        prepared_count: usize,
    },
    Prepared {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Activated {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Aborted {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Degraded {
        operation: GatewayOperation,
        error_code: ErrorCode,
    },
    RequestStarted {
        operation: GatewayRequestOperation,
        metadata: GatewayRequestMetadata,
    },
    RequestCompleted {
        operation: GatewayRequestOperation,
        metadata: GatewayRequestMetadata,
        outcome: GatewayRequestOutcome,
        elapsed_micros: u64,
    },
    TransportStarting {
        listen_address: String,
        worker_count: u32,
    },
    ShutdownStarted {
        drain_millis: u64,
        reason: String,
    },
    ShutdownCompleted,
}

/// Driven observability port. Implementations should remain lightweight;
/// buffering and delivery belong to runtime adapters.
pub trait GatewayEventSink: Send + Sync {
    fn emit(&self, event: &GatewayEvent);
}

#[derive(Default)]
pub struct NoopGatewayEventSink;

impl GatewayEventSink for NoopGatewayEventSink {
    fn emit(&self, _event: &GatewayEvent) {}
}
