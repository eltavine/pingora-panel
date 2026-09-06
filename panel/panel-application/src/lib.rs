#![forbid(unsafe_code)]

//! Transport and storage neutral application ports for the control plane.
//!
//! This crate deliberately does not depend on `panel-engine`, Pingora, Tonic,
//! Axum, SQLx, or a GUI. Adapters own those dependencies and translate at the
//! boundary. The types here are the control-plane contract used by REST, CLI,
//! workers, and tests.

use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use serde::{Deserialize, Deserializer, Serialize};

/// A caller supplied key that makes a mutating command safe to retry.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Creates a key after rejecting empty or excessively large input.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 {
            return Err(PanelError::invalid_argument(
                "idempotency key must contain 1..=256 bytes",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the stable wire representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// An application request identifier kept separate from idempotency identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    /// Creates a bounded request identifier.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 {
            return Err(PanelError::invalid_argument(
                "request id must contain 1..=256 bytes",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the stable wire representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Neutral result of preparing a gateway snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedDeployment {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub prepare_token: String,
}

/// Neutral result of activating a prepared snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivatedDeployment {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub previous_active_hash: Option<ContentHash>,
}

/// Outcome used when activation may have happened but its receipt is unknown.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeploymentOutcome {
    Succeeded(ActivatedDeployment),
    Rejected(ValidationReport),
    FailedBeforeCommit,
    PendingReconciliation,
}

/// Application owned gateway port. Its implementation may call gRPC, a fake,
/// or another data-plane adapter without leaking that choice into this crate.
#[async_trait]
pub trait GatewayPort: Send + Sync {
    /// Validates a snapshot against the selected data-plane capabilities.
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport>;

    /// Prepares a snapshot without publishing it to traffic.
    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment>;

    /// Activates a previously prepared snapshot with an optional CAS guard.
    async fn activate(
        &self,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment>;
}

/// The revision persistence port. SQLx, filesystem, and test implementations
/// all map their own records to these transport-neutral values.
#[async_trait]
pub trait RevisionRepository: Send + Sync {
    /// Saves a draft revision and returns its assigned identifier.
    async fn save_draft(&self, snapshot: RuntimeSnapshot) -> Result<RevisionId>;

    /// Loads a revision by identifier.
    async fn load(&self, revision_id: RevisionId) -> Result<RuntimeSnapshot>;

    /// Records the terminal or pending outcome for an idempotent deployment.
    async fn record_outcome(
        &self,
        idempotency_key: &IdempotencyKey,
        outcome: &DeploymentOutcome,
    ) -> Result<()>;
}

/// Immutable audit fact emitted by a successful or rejected application command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFact {
    pub event_type: String,
    pub event_version: u16,
    pub request_id: RequestId,
    pub idempotency_key: Option<IdempotencyKey>,
    pub revision_id: Option<RevisionId>,
    pub content_hash: Option<ContentHash>,
    pub outcome: String,
}

/// Atomic audit/outbox port. A database adapter implements this with one local
/// transaction while keeping the application independent from SQLx. Keeping
/// the operation atomic at this boundary prevents a successful deployment from
/// being reported without its audit fact or outbox event.
#[async_trait]
pub trait AuditEventStore: Send + Sync {
    /// Appends an immutable audit fact and its corresponding event atomically.
    async fn append_audit_and_event(&self, fact: AuditFact) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_keys_have_a_bounded_wire_shape() {
        assert!(IdempotencyKey::new("deploy-1").is_ok());
        assert!(IdempotencyKey::new("").is_err());
        assert!(IdempotencyKey::new("x".repeat(257)).is_err());
        assert!(RequestId::new("request-1").is_ok());
        assert!(serde_json::from_str::<IdempotencyKey>("\"\"").is_err());
    }
}
