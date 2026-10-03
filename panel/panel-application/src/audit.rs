//! The audit trail as the API reads it.

use crate::RequestScope;
use async_trait::async_trait;
use panel_errors::Result;
use serde_json::Value;
use std::time::SystemTime;

/// One audited event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuditRecord {
    /// Position in the hash chain, from 1.
    pub sequence: u64,
    pub event_id: String,
    pub source: String,
    pub event_type: String,
    pub event_version: u32,
    pub subject: String,
    pub occurred_at: Option<SystemTime>,
    pub recorded_at: Option<SystemTime>,
    pub actor_type: String,
    pub actor_id: String,
    pub correlation_id: String,
    pub causation_id: String,
    pub idempotency_key: String,
    pub traceparent: String,
    pub data: Value,
    pub hash: String,
    pub previous_hash: String,
}

/// Which records to list; empty values match everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditFilter {
    /// Only records older than this sequence.
    pub before: Option<u64>,
    /// At most this many; zero for the default.
    pub limit: u32,
    pub actor_id: String,
    /// An event type, or a prefix ending in `.`.
    pub event_type: String,
    pub subject: String,
    pub correlation_id: String,
    pub since: Option<SystemTime>,
    pub until: Option<SystemTime>,
}

/// Records newest first, and where the next page starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuditPage {
    pub records: Vec<AuditRecord>,
    pub next_before: Option<u64>,
}

/// The result of verifying the hash chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditVerification {
    pub intact: bool,
    pub checked: u64,
    pub first_mismatch: Option<u64>,
    pub head_sequence: u64,
    pub head_hash: String,
}

/// Reads the audit trail.
#[async_trait]
pub trait AuditPort: Send + Sync {
    async fn list(&self, scope: RequestScope, filter: AuditFilter) -> Result<AuditPage>;

    async fn get(&self, scope: RequestScope, sequence: u64) -> Result<AuditRecord>;

    /// Verifies records `from..=to`, the whole chain by default.
    async fn verify(
        &self,
        scope: RequestScope,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<AuditVerification>;
}
