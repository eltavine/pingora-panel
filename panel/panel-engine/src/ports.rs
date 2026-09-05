use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct EngineCapability {
    pub name: String,
    pub version: String,
}

impl EngineCapability {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EngineCapabilities {
    pub protocol_version: String,
    pub build_version: String,
    pub schema_version: String,
    pub adapter_version: String,
    pub capabilities: BTreeSet<EngineCapability>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrepareToken(String);

impl PrepareToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    pub snapshot: RuntimeSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedSnapshotRecord {
    pub envelope: SnapshotEnvelope,
    pub receipt: PrepareReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActiveSnapshotRecord {
    pub envelope: SnapshotEnvelope,
    pub receipt: ActivationReceipt,
}

/// Result of publishing an activation record to durable storage.
///
/// `DurabilityUnknown` means the record has already replaced the previous
/// value in the current filesystem namespace, but synchronizing the directory
/// failed. Callers must reconcile their in-memory state with the published
/// record instead of treating this as a pre-commit failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum ActivationCommitOutcome {
    Committed,
    DurabilityUnknown(PanelError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepareRequest {
    pub snapshot: RuntimeSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateRequest {
    pub prepare_token: PrepareToken,
    pub expected_active_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrepareReceipt {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub adapter_version: String,
    pub schema_version: String,
    pub prepare_token: PrepareToken,
    pub previous_active_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivationReceipt {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub adapter_version: String,
    pub schema_version: String,
    pub prepare_token: PrepareToken,
    pub previous_active_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AbortReceipt {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub adapter_version: String,
    pub schema_version: String,
    pub prepare_token: PrepareToken,
    pub previous_active_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayStatus {
    pub ready: bool,
    pub message: Option<String>,
    pub active_revision_id: Option<RevisionId>,
    pub active_hash: Option<ContentHash>,
    pub prepared_count: usize,
    pub adapter_version: String,
    pub schema_version: String,
}

/// Process facts exposed alongside engine status without coupling the engine to
/// clocks, environment variables, executors, or a particular transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayRuntimeInfo {
    pub gateway_version: String,
    pub started_at_unix_seconds: u64,
    pub uptime_seconds: u64,
    pub worker_count: u32,
}

/// Read-only driven port for process metadata.
///
/// Composition roots provide the implementation. Application runtimes and data
/// plane adapters therefore remain unaware of operating-system process details.
pub trait GatewayRuntimeInfoProvider: Send + Sync {
    fn snapshot(&self) -> GatewayRuntimeInfo;
}

/// Transport-neutral counters describing loss or isolation in the gateway event
/// delivery path. Private fields keep additive evolution source-compatible.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayEventDeliveryDiagnostics {
    queue_full_events: u64,
    disconnected_events: u64,
    consumer_panics: u64,
}

impl GatewayEventDeliveryDiagnostics {
    pub const fn new(
        queue_full_events: u64,
        disconnected_events: u64,
        consumer_panics: u64,
    ) -> Self {
        Self {
            queue_full_events,
            disconnected_events,
            consumer_panics,
        }
    }

    pub const fn queue_full_events(self) -> u64 {
        self.queue_full_events
    }

    pub const fn disconnected_events(self) -> u64 {
        self.disconnected_events
    }

    pub const fn dropped_events(self) -> u64 {
        self.queue_full_events
            .saturating_add(self.disconnected_events)
    }

    pub const fn consumer_panics(self) -> u64 {
        self.consumer_panics
    }
}

/// Read-only driven port for event-delivery diagnostics.
///
/// Transports depend on this stable projection rather than a queue, metrics
/// implementation, or runtime adapter.
pub trait GatewayEventDeliveryDiagnosticsProvider: Send + Sync {
    fn snapshot(&self) -> GatewayEventDeliveryDiagnostics;
}

/// Transport-neutral recovery counters. Private fields and a constructor keep
/// additive evolution source-compatible across runtime and transport adapters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayRecoveryDiagnostics {
    recovery_completed: u64,
    degraded_events: u64,
    unknown_commit_outcomes: u64,
}

impl GatewayRecoveryDiagnostics {
    pub const fn new(
        recovery_completed: u64,
        degraded_events: u64,
        unknown_commit_outcomes: u64,
    ) -> Self {
        Self {
            recovery_completed,
            degraded_events,
            unknown_commit_outcomes,
        }
    }

    pub const fn recovery_completed(self) -> u64 {
        self.recovery_completed
    }

    pub const fn degraded_events(self) -> u64 {
        self.degraded_events
    }

    pub const fn unknown_commit_outcomes(self) -> u64 {
        self.unknown_commit_outcomes
    }
}

/// Read-only driven port for recovery diagnostics.
pub trait GatewayRecoveryDiagnosticsProvider: Send + Sync {
    fn recovery_snapshot(&self) -> GatewayRecoveryDiagnostics;
}

#[async_trait]
pub trait GatewayEngine: Send + Sync {
    async fn capabilities(&self) -> Result<EngineCapabilities>;
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport>;
    async fn prepare(&self, request: PrepareRequest) -> Result<PrepareReceipt>;
    async fn activate(&self, request: ActivateRequest) -> Result<ActivationReceipt>;
    async fn abort(&self, token: PrepareToken) -> Result<AbortReceipt>;
    async fn status(&self) -> Result<GatewayStatus>;
}

/// Engine-specific snapshot compiler and atomic data-plane switch.
///
/// `prepare` may fail and must perform every fallible engine operation. `activate`
/// is intentionally infallible: implementations publish a fully prepared immutable
/// value with a single in-memory pointer swap. This lets the durable runtime persist
/// the activation record before exposing the new snapshot to traffic.
#[async_trait]
pub trait DataPlaneAdapter: Send + Sync {
    type Prepared: Send + Sync + 'static;

    async fn capabilities(&self) -> Result<EngineCapabilities>;
    async fn validate(&self, snapshot: &RuntimeSnapshot) -> Result<ValidationReport>;
    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<Self::Prepared>;
    fn activate(&self, prepared: Arc<Self::Prepared>);
}

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    async fn load_active(&self) -> Result<Option<ActiveSnapshotRecord>>;
    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>>;

    /// Load no more than `limit` prepared records.
    ///
    /// The default preserves source compatibility for existing adapters. Stores
    /// that can reject an oversized collection before materializing every record
    /// should override this method.
    async fn load_prepared_bounded(&self, limit: usize) -> Result<Vec<PreparedSnapshotRecord>> {
        let records = self.load_prepared().await?;
        if records.len() > limit {
            return Err(panel_errors::PanelError::resource_exhausted(format!(
                "snapshot store contains more than {limit} prepared records"
            )));
        }
        Ok(records)
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()>;
    async fn delete_prepared(&self, token: &PrepareToken) -> Result<()>;
    async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()>;

    /// Commit an activation while preserving an ambiguous post-publication
    /// durability outcome.
    ///
    /// The default keeps existing adapters source-compatible: stores without a
    /// richer durability model report a successful commit as `Committed`.
    async fn commit_activation_with_outcome(
        &self,
        record: ActiveSnapshotRecord,
    ) -> Result<ActivationCommitOutcome> {
        self.commit_activation(record).await?;
        Ok(ActivationCommitOutcome::Committed)
    }
}
