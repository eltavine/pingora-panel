use crate::{DeploymentOutcome, IdempotencyKey, RequestId};
use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::Result;
use panel_ir::RuntimeSnapshot;
use serde::{Deserialize, Serialize};

/// Durable result associated with one idempotency key and request fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdempotencyRecord {
    request_hash: ContentHash,
    outcome: DeploymentOutcome,
}

impl IdempotencyRecord {
    pub fn new(request_hash: ContentHash, outcome: DeploymentOutcome) -> Self {
        Self {
            request_hash,
            outcome,
        }
    }

    pub fn request_hash(&self) -> &ContentHash {
        &self.request_hash
    }

    pub fn outcome(&self) -> &DeploymentOutcome {
        &self.outcome
    }
}

/// Result of atomically claiming an idempotency key.
///
/// This enum is intentionally non-exhaustive: persistence adapters may gain
/// additional intermediate states (for example, an expired lease) without
/// forcing downstream consumers to release a breaking change.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IdempotencyClaim {
    Acquired,
    Replay(IdempotencyRecord),
    InProgress,
    Conflict,
}

/// Atomic idempotency boundary. Implementations must make `claim`
/// linearizable so concurrent retries cannot execute a mutation twice.
#[async_trait]
pub trait IdempotencyRepository: Send + Sync {
    async fn claim(
        &self,
        key: &IdempotencyKey,
        request_hash: &ContentHash,
    ) -> Result<IdempotencyClaim>;

    /// Publishes the replay receipt for a previously acquired claim.
    ///
    /// Implementations must make this transition durable and linearizable with
    /// `claim`; a failure must not implicitly release the claim.
    async fn complete(&self, key: &IdempotencyKey, record: IdempotencyRecord) -> Result<()>;

    /// Releases a claim only when the gateway failed before commit.
    async fn abort(&self, key: &IdempotencyKey, request_hash: &ContentHash) -> Result<()>;
}

/// Persistence port for versioned configuration revisions.
#[async_trait]
pub trait RevisionRepository: Send + Sync {
    async fn save_draft(&self, snapshot: RuntimeSnapshot) -> Result<RevisionId>;

    async fn load(&self, revision_id: RevisionId) -> Result<RuntimeSnapshot>;

    async fn record_outcome(
        &self,
        idempotency_key: &IdempotencyKey,
        outcome: &DeploymentOutcome,
    ) -> Result<()>;
}

/// Immutable audit fact emitted by an application command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditFact {
    event_type: String,
    event_version: u16,
    request_id: RequestId,
    idempotency_key: Option<IdempotencyKey>,
    revision_id: Option<RevisionId>,
    content_hash: Option<ContentHash>,
    outcome: String,
}

impl AuditFact {
    pub fn new(
        event_type: impl Into<String>,
        event_version: u16,
        request_id: RequestId,
        outcome: impl Into<String>,
    ) -> Result<Self> {
        let event_type = event_type.into();
        let outcome = outcome.into();
        if event_type.is_empty() || event_type.len() > 128 {
            return Err(panel_errors::PanelError::invalid_argument(
                "audit event_type must contain 1..=128 bytes",
            ));
        }
        if outcome.is_empty() || outcome.len() > 128 {
            return Err(panel_errors::PanelError::invalid_argument(
                "audit outcome must contain 1..=128 bytes",
            ));
        }
        Ok(Self {
            event_type,
            event_version,
            request_id,
            idempotency_key: None,
            revision_id: None,
            content_hash: None,
            outcome,
        })
    }

    pub fn with_idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    pub fn with_revision_id(mut self, revision_id: RevisionId) -> Self {
        self.revision_id = Some(revision_id);
        self
    }

    pub fn with_content_hash(mut self, content_hash: ContentHash) -> Self {
        self.content_hash = Some(content_hash);
        self
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn event_version(&self) -> u16 {
        self.event_version
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    pub fn revision_id(&self) -> Option<RevisionId> {
        self.revision_id
    }

    pub fn content_hash(&self) -> Option<&ContentHash> {
        self.content_hash.as_ref()
    }

    pub fn outcome(&self) -> &str {
        &self.outcome
    }
}

/// Atomic audit/outbox boundary implemented by a persistence adapter.
#[async_trait]
pub trait AuditEventStore: Send + Sync {
    async fn append_audit_and_event(&self, fact: AuditFact) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_fact_uses_extensible_constructor_and_accessors() {
        let request_id = RequestId::new("req-1").unwrap();
        let key = IdempotencyKey::new("idem-1").unwrap();
        let fact = AuditFact::new("deployment", 1, request_id, "succeeded")
            .unwrap()
            .with_idempotency_key(key);

        assert_eq!(fact.event_type(), "deployment");
        assert_eq!(fact.event_version(), 1);
        assert_eq!(fact.outcome(), "succeeded");
        assert_eq!(fact.idempotency_key().unwrap().as_str(), "idem-1");
    }

    #[test]
    fn audit_fact_rejects_unbounded_labels() {
        let request_id = RequestId::new("req-1").unwrap();
        let error = AuditFact::new("", 1, request_id, "succeeded").unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }
}
