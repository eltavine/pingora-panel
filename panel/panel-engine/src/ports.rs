use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{Result, ValidationReport};
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
}
