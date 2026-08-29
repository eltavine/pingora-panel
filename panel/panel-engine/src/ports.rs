use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
    pub active_revision_id: Option<RevisionId>,
    pub active_hash: Option<ContentHash>,
    pub prepared_count: usize,
    pub adapter_version: String,
    pub schema_version: String,
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

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    async fn load_last_known_good(&self) -> Result<Option<SnapshotEnvelope>>;
    async fn save_prepared(&self, snapshot: SnapshotEnvelope) -> Result<()>;
    async fn save_activation_receipt(&self, receipt: ActivationReceipt) -> Result<()>;
}
