//! What the configuration use cases keep and record, behind ports a store
//! implements: the draft, the revision history, approvals, and the events
//! beside them. A store records the events of its own changes with them.

use crate::approval_rules::{Bypass, Gate, Opening};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_application::{ContentHash, IdempotencyKey, RequestScope};
use panel_config_dsl::Sources;
use panel_config_model::{
    ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ConfigModel, Revision,
};
use panel_errors::{Diagnostic, Result};
use panel_events::{AggregateId, AggregateRef, AggregateType, EventData, EventDraft};
use uuid::Uuid;

/// The aggregate of draft events.
pub const DRAFT: (&str, &str) = ("configuration", "draft");

/// The draft and its application state.
#[derive(Clone, Debug)]
pub struct DraftState {
    pub version: u64,
    pub model: ConfigModel,
    /// The draft in the configuration language.
    pub sources: Sources,
    pub updated_at: DateTime<Utc>,
    pub applied_version: Option<u64>,
    pub applied_at: Option<DateTime<Utc>>,
}

/// What one change produced; replayed for a repeated idempotency key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeOutput {
    pub content: Vec<u8>,
    pub etag: String,
}

/// A change's new draft, as a model and in the configuration language.
pub struct DraftChange {
    pub model: ConfigModel,
    pub sources: Sources,
    pub output: ChangeOutput,
}

/// Identifies one change for idempotency and events.
pub struct ChangeRequest<'a> {
    pub idempotency_key: &'a IdempotencyKey,
    pub operation: &'a str,
    pub resource: &'a str,
    /// Hash of everything the change asks, so a repeated key can be told
    /// from a reused one.
    pub request_hash: ContentHash,
    pub scope: &'a RequestScope,
    pub actor: &'a str,
}

/// Computes a change from the current draft, within the store's update.
pub type DraftEdit<'a> = Box<dyn FnOnce(&DraftState) -> Result<DraftChange> + Send + 'a>;

/// The draft document.
#[async_trait]
pub trait DraftStore: Send + Sync {
    /// The draft; the empty one before the first change.
    async fn load(&self) -> Result<DraftState>;

    /// Applies `edit` to the current draft as one update that no other
    /// change interleaves with. A repeated idempotency key returns the
    /// recorded output when the request is the same and is refused when it
    /// differs.
    async fn change(
        &self,
        request: ChangeRequest<'_>,
        edit: DraftEdit<'_>,
    ) -> Result<(DraftState, ChangeOutput)>;

    /// Records that `version`, as `revision`, now runs on the gateway.
    async fn mark_applied(
        &self,
        version: u64,
        revision: u64,
        note: Option<&str>,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<DraftState>;
}

/// A revision to record.
pub struct NewRevision<'a> {
    pub draft_version: u64,
    pub sources: &'a Sources,
    pub content_hash: &'a str,
    pub author: &'a str,
    pub note: Option<&'a str>,
    pub snapshot_hash: Option<&'a str>,
}

/// Every configuration applied or attempted: its files, author, note and
/// outcome. Files never change once recorded.
#[async_trait]
pub trait RevisionStore: Send + Sync {
    /// Records an attempt about to reach the gateway.
    async fn begin(&self, revision: &NewRevision<'_>) -> Result<u64>;

    /// Records an attempt that failed validation.
    async fn reject(&self, revision: &NewRevision<'_>, diagnostics: &[Diagnostic]) -> Result<u64>;

    /// Marks `id` as what the gateway runs; the previous one is superseded.
    async fn activate(&self, id: u64, snapshot_hash: &str, gateway_revision: u64) -> Result<()>;

    /// Marks an attempt still applying as failed.
    async fn fail(&self, id: u64, diagnostics: &[Diagnostic]) -> Result<()>;

    /// Settles attempts a crash left open: the one whose snapshot the gateway
    /// runs becomes active, the others failed.
    async fn settle(&self, active_hash: Option<&str>) -> Result<()>;

    async fn get(&self, id: u64) -> Result<(Revision, Sources)>;

    /// The revision the gateway runs, if any.
    async fn active(&self) -> Result<Option<(Revision, Sources)>>;

    /// Revisions newest first, older than `before` when given.
    async fn list(&self, before: Option<u64>, limit: u32) -> Result<Vec<Revision>>;

    async fn set_note(&self, id: u64, note: Option<&str>) -> Result<Revision>;
}

/// Approval policies, the requests applying covered changes opens, and the
/// decisions on them (ADR 0019). Each decision follows
/// [`approval_rules`](crate::approval_rules) within one update of the store.
#[async_trait]
pub trait ApprovalStore: Send + Sync {
    async fn policies(&self) -> Result<Vec<ApprovalPolicy>>;

    async fn policy(&self, id: &str) -> Result<ApprovalPolicy>;

    /// Creates or replaces a policy; true when it is new.
    async fn put_policy(
        &self,
        id: &str,
        input: ApprovalPolicyInput,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(ApprovalPolicy, bool)>;

    async fn delete_policy(&self, id: &str, scope: &RequestScope, actor: &str) -> Result<()>;

    /// Requests newest first, as stored.
    async fn requests(
        &self,
        before: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<ApprovalRequest>>;

    async fn request(&self, id: Uuid) -> Result<ApprovalRequest>;

    /// Decides what applying the opening's content needs, closing the open
    /// requests that no longer fit and opening one when none does.
    async fn gate(
        &self,
        opening: Opening<'_>,
        now: DateTime<Utc>,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Gate>;

    /// Records `approver`'s approval of content the draft still has.
    async fn approve(
        &self,
        id: Uuid,
        approver: &str,
        draft_hash: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest>;

    /// Rejects an open request someone else asked for.
    async fn reject(
        &self,
        id: Uuid,
        approver: &str,
        reason: Option<&str>,
        draft_hash: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest>;

    /// Withdraws a request its requester no longer wants.
    async fn withdraw(
        &self,
        id: Uuid,
        actor: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest>;

    /// Takes back `approver`'s approval of a request not yet applied.
    async fn revoke(
        &self,
        id: Uuid,
        approver: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest>;

    /// Records that the approved request was applied as `revision`.
    async fn applied(
        &self,
        id: Uuid,
        revision: u64,
        actor: &str,
        scope: &RequestScope,
    ) -> Result<()>;

    /// Records an emergency bypass before anything is published, closing
    /// open requests for the same content; nothing may be applied unless
    /// this is recorded.
    async fn bypassed(
        &self,
        bypass: &Bypass,
        content_hash: &str,
        covering: &[&ApprovalPolicy],
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()>;
}

/// Records what happened beside the stores' changes, such as a refused
/// change or how an apply ended. It already happened, so recording cannot
/// fail it.
#[async_trait]
pub trait EventRecorder: Send + Sync {
    async fn record(&self, event: EventDraft, scope: &RequestScope, actor: &str);
}

/// Records `data` about `aggregate`, a type and an ID.
pub(crate) async fn record<E: EventData + Sync>(
    recorder: &dyn EventRecorder,
    (kind, id): (&str, &str),
    scope: &RequestScope,
    actor: &str,
    data: &E,
) {
    let event = AggregateType::new(kind)
        .and_then(|kind| Ok(AggregateRef::new(kind, AggregateId::new(id)?)))
        .and_then(|aggregate| EventDraft::of(aggregate, data));
    match event {
        Ok(event) => recorder.record(event, scope, actor).await,
        Err(error) => {
            tracing::warn!(error_code = %error.code, event_type = E::TYPE, "event not recorded");
        }
    }
}
