//! Stores that keep everything in memory, so the configuration use cases can
//! run without a database or a transport.

use crate::{
    approval_rules::{self as rules, Bypass, Gate, Opening},
    language,
    store::{
        ApprovalStore, ChangeOutput, ChangeRequest, DraftChange, DraftEdit, DraftState, DraftStore,
        EventRecorder, NewRevision, RevisionStore,
    },
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_application::{
    AbortOutcome, ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayStatus,
    GatewayUseCases, PreparedDeployment, RequestScope,
};
use panel_config_dsl::{Sources, LANGUAGE_VERSION};
use panel_config_model::{
    ApprovalDecision, ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ApprovalState,
    ConfigModel, Revision, RevisionOutcome,
};
use panel_errors::{Diagnostic, PanelError, Result, ValidationReport};
use panel_events::EventDraft;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, MutexGuard, PoisonError},
};
use uuid::Uuid;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Each idempotency key's request hash and output.
type Receipts = HashMap<String, (String, ChangeOutput)>;

pub(crate) struct MemoryDrafts {
    state: Mutex<(DraftState, Receipts)>,
}

impl Default for MemoryDrafts {
    fn default() -> Self {
        let model = ConfigModel::default();
        let draft = DraftState {
            version: 0,
            sources: language::printed(&model),
            model,
            updated_at: Utc::now(),
            applied_version: None,
            applied_at: None,
        };
        Self {
            state: Mutex::new((draft, HashMap::new())),
        }
    }
}

#[async_trait]
impl DraftStore for MemoryDrafts {
    async fn load(&self) -> Result<DraftState> {
        Ok(locked(&self.state).0.clone())
    }

    async fn change(
        &self,
        request: ChangeRequest<'_>,
        edit: DraftEdit<'_>,
    ) -> Result<(DraftState, ChangeOutput)> {
        let mut state = locked(&self.state);
        let (draft, receipts) = &mut *state;
        if let Some((hash, output)) = receipts.get(request.idempotency_key.as_str()) {
            if hash != request.request_hash.as_str() {
                return Err(PanelError::conflict(
                    "the idempotency key was already used for a different change",
                ));
            }
            return Ok((draft.clone(), output.clone()));
        }
        let DraftChange {
            model,
            sources,
            output,
        } = edit(draft)?;
        *draft = DraftState {
            version: draft.version + 1,
            model,
            sources,
            updated_at: Utc::now(),
            ..draft.clone()
        };
        receipts.insert(
            request.idempotency_key.as_str().to_owned(),
            (request.request_hash.as_str().to_owned(), output.clone()),
        );
        Ok((draft.clone(), output))
    }

    async fn mark_applied(
        &self,
        version: u64,
        _revision: u64,
        _note: Option<&str>,
        _scope: &RequestScope,
        _actor: &str,
    ) -> Result<DraftState> {
        let mut state = locked(&self.state);
        let draft = &mut state.0;
        if draft
            .applied_version
            .is_none_or(|applied| applied < version)
        {
            draft.applied_version = Some(version);
            draft.applied_at = Some(Utc::now());
        }
        Ok(draft.clone())
    }
}

#[derive(Default)]
pub(crate) struct MemoryRevisions {
    revisions: Mutex<Vec<(Revision, Sources)>>,
}

impl MemoryRevisions {
    fn record(
        &self,
        revision: &NewRevision<'_>,
        outcome: RevisionOutcome,
        diagnostics: &[Diagnostic],
    ) -> u64 {
        let mut revisions = locked(&self.revisions);
        let id = revisions.len() as u64 + 1;
        let now = Utc::now();
        revisions.push((
            Revision {
                id,
                draft_version: revision.draft_version,
                language_version: LANGUAGE_VERSION,
                content_hash: revision.content_hash.to_owned(),
                author: revision.author.to_owned(),
                note: revision.note.map(str::to_owned),
                created_at: now,
                outcome,
                outcome_at: now,
                diagnostics: diagnostics.to_vec(),
                snapshot_hash: revision.snapshot_hash.map(str::to_owned),
                gateway_revision: None,
            },
            revision.sources.clone(),
        ));
        id
    }

    fn settle(revisions: &mut [(Revision, Sources)], id: u64, outcome: RevisionOutcome) {
        let now = Utc::now();
        if outcome == RevisionOutcome::Active {
            for (revision, _) in revisions.iter_mut() {
                if revision.outcome == RevisionOutcome::Active && revision.id != id {
                    revision.outcome = RevisionOutcome::Superseded;
                    revision.outcome_at = now;
                }
            }
        }
        if let Some((revision, _)) = revisions.iter_mut().find(|(revision, _)| revision.id == id) {
            revision.outcome = outcome;
            revision.outcome_at = now;
        }
    }
}

#[async_trait]
impl RevisionStore for MemoryRevisions {
    async fn begin(&self, revision: &NewRevision<'_>) -> Result<u64> {
        Ok(self.record(revision, RevisionOutcome::Applying, &[]))
    }

    async fn reject(&self, revision: &NewRevision<'_>, diagnostics: &[Diagnostic]) -> Result<u64> {
        Ok(self.record(revision, RevisionOutcome::Rejected, diagnostics))
    }

    async fn activate(&self, id: u64, snapshot_hash: &str, gateway_revision: u64) -> Result<()> {
        let mut revisions = locked(&self.revisions);
        Self::settle(&mut revisions, id, RevisionOutcome::Active);
        if let Some((revision, _)) = revisions.iter_mut().find(|(revision, _)| revision.id == id) {
            revision.snapshot_hash = Some(snapshot_hash.to_owned());
            revision.gateway_revision = Some(gateway_revision);
        }
        Ok(())
    }

    async fn fail(&self, id: u64, diagnostics: &[Diagnostic]) -> Result<()> {
        let mut revisions = locked(&self.revisions);
        if let Some((revision, _)) = revisions.iter_mut().find(|(revision, _)| {
            revision.id == id && revision.outcome == RevisionOutcome::Applying
        }) {
            revision.outcome = RevisionOutcome::Failed;
            revision.diagnostics = diagnostics.to_vec();
        }
        Ok(())
    }

    async fn settle(&self, active_hash: Option<&str>) -> Result<()> {
        let mut revisions = locked(&self.revisions);
        let open: Vec<(u64, bool)> = revisions
            .iter()
            .filter(|(revision, _)| revision.outcome == RevisionOutcome::Applying)
            .map(|(revision, _)| {
                (
                    revision.id,
                    revision.snapshot_hash.is_some()
                        && revision.snapshot_hash.as_deref() == active_hash,
                )
            })
            .collect();
        for (id, runs) in open {
            let outcome = if runs {
                RevisionOutcome::Active
            } else {
                RevisionOutcome::Failed
            };
            Self::settle(&mut revisions, id, outcome);
        }
        Ok(())
    }

    async fn get(&self, id: u64) -> Result<(Revision, Sources)> {
        locked(&self.revisions)
            .iter()
            .find(|(revision, _)| revision.id == id)
            .cloned()
            .ok_or_else(|| PanelError::not_found(format!("no revision {id}")))
    }

    async fn active(&self) -> Result<Option<(Revision, Sources)>> {
        Ok(locked(&self.revisions)
            .iter()
            .find(|(revision, _)| revision.outcome == RevisionOutcome::Active)
            .cloned())
    }

    async fn list(&self, before: Option<u64>, limit: u32) -> Result<Vec<Revision>> {
        Ok(locked(&self.revisions)
            .iter()
            .rev()
            .filter(|(revision, _)| before.is_none_or(|before| revision.id < before))
            .take(limit as usize)
            .map(|(revision, _)| revision.clone())
            .collect())
    }

    async fn set_note(&self, id: u64, note: Option<&str>) -> Result<Revision> {
        let mut revisions = locked(&self.revisions);
        let (revision, _) = revisions
            .iter_mut()
            .find(|(revision, _)| revision.id == id)
            .ok_or_else(|| PanelError::not_found(format!("no revision {id}")))?;
        revision.note = note.map(str::to_owned);
        Ok(revision.clone())
    }
}

#[derive(Default)]
pub(crate) struct MemoryApprovals {
    policies: Mutex<BTreeMap<String, ApprovalPolicy>>,
    /// Oldest first.
    requests: Mutex<Vec<ApprovalRequest>>,
}

impl MemoryApprovals {
    fn decide(
        &self,
        id: Uuid,
        decide: impl FnOnce(&mut ApprovalRequest) -> Result<()>,
    ) -> Result<ApprovalRequest> {
        let mut requests = locked(&self.requests);
        let request = requests
            .iter_mut()
            .find(|request| request.id == id)
            .ok_or_else(|| PanelError::not_found(format!("there is no approval request {id}")))?;
        decide(request)?;
        Ok(request.clone())
    }

    fn close(
        request: &mut ApprovalRequest,
        state: ApprovalState,
        actor: &str,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) {
        request.state = state;
        request.closed_by = Some(actor.to_owned());
        request.closed_at = Some(now);
        request.reason = reason.map(str::to_owned);
    }
}

#[async_trait]
impl ApprovalStore for MemoryApprovals {
    async fn policies(&self) -> Result<Vec<ApprovalPolicy>> {
        Ok(locked(&self.policies).values().cloned().collect())
    }

    async fn policy(&self, id: &str) -> Result<ApprovalPolicy> {
        locked(&self.policies)
            .get(id)
            .cloned()
            .ok_or_else(|| PanelError::not_found(format!("there is no approval policy {id}")))
    }

    async fn put_policy(
        &self,
        id: &str,
        input: ApprovalPolicyInput,
        _scope: &RequestScope,
        _actor: &str,
    ) -> Result<(ApprovalPolicy, bool)> {
        let problems = input.problems(id);
        if !problems.is_empty() {
            return Err(PanelError::validation_failed(problems.join("; ")));
        }
        let now = Utc::now();
        let mut policies = locked(&self.policies);
        let created = !policies.contains_key(id);
        let policy = policies
            .entry(id.to_owned())
            .and_modify(|policy| {
                policy.policy = input.clone();
                policy.version += 1;
                policy.updated_at = now;
            })
            .or_insert_with(|| ApprovalPolicy {
                id: id.to_owned(),
                policy: input,
                version: 1,
                created_at: now,
                updated_at: now,
            });
        Ok((policy.clone(), created))
    }

    async fn delete_policy(&self, id: &str, _scope: &RequestScope, _actor: &str) -> Result<()> {
        locked(&self.policies)
            .remove(id)
            .map(drop)
            .ok_or_else(|| PanelError::not_found(format!("there is no approval policy {id}")))
    }

    async fn requests(
        &self,
        before: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<ApprovalRequest>> {
        Ok(locked(&self.requests)
            .iter()
            .rev()
            .filter(|request| before.is_none_or(|before| request.requested_at < before))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn request(&self, id: Uuid) -> Result<ApprovalRequest> {
        self.decide(id, |_| Ok(()))
    }

    async fn gate(
        &self,
        opening: Opening<'_>,
        now: DateTime<Utc>,
        _scope: &RequestScope,
        actor: &str,
    ) -> Result<Gate> {
        if opening.covering.is_empty() {
            return Ok(Gate::Clear);
        }
        let mut requests = locked(&self.requests);
        let open = requests
            .iter()
            .rev()
            .filter(|request| {
                request.content_hash == opening.content_hash
                    && matches!(
                        request.state,
                        ApprovalState::Pending | ApprovalState::Approved
                    )
            })
            .cloned()
            .collect();
        let screening = rules::screen(&opening, open, now);
        for request in requests.iter_mut() {
            if screening.outdated.contains(&request.id) {
                Self::close(request, ApprovalState::Outdated, actor, None, now);
            }
        }
        if let Some(gate) = screening.gate {
            return Ok(gate);
        }
        let created = rules::open_request(opening, actor, now);
        requests.push(created.clone());
        Ok(Gate::Awaiting(Box::new(created)))
    }

    async fn approve(
        &self,
        id: Uuid,
        approver: &str,
        draft_hash: &str,
        now: DateTime<Utc>,
        _scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        self.decide(id, |request| {
            let approval = rules::approval(request, approver, draft_hash, now)?;
            request.approvals.retain(|given| given.approver != approver);
            request.approvals.push(ApprovalDecision {
                approver: approver.to_owned(),
                approved_at: now,
                valid_until: approval.valid_until,
                revoked_at: None,
            });
            if approval.completes {
                request.state = ApprovalState::Approved;
            }
            Ok(())
        })
    }

    async fn reject(
        &self,
        id: Uuid,
        approver: &str,
        reason: Option<&str>,
        draft_hash: &str,
        now: DateTime<Utc>,
        _scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        self.decide(id, |request| {
            rules::check_rejection(request, approver, draft_hash, now)?;
            Self::close(request, ApprovalState::Rejected, approver, reason, now);
            Ok(())
        })
    }

    async fn withdraw(
        &self,
        id: Uuid,
        actor: &str,
        now: DateTime<Utc>,
        _scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        self.decide(id, |request| {
            rules::check_withdrawal(request, actor)?;
            Self::close(request, ApprovalState::Withdrawn, actor, None, now);
            Ok(())
        })
    }

    async fn revoke(
        &self,
        id: Uuid,
        approver: &str,
        now: DateTime<Utc>,
        _scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        self.decide(id, |request| {
            let reopens = rules::revocation(request, approver, now)?;
            for given in &mut request.approvals {
                if given.approver == approver && given.revoked_at.is_none() {
                    given.revoked_at = Some(now);
                }
            }
            if reopens {
                request.state = ApprovalState::Pending;
            }
            Ok(())
        })
    }

    async fn applied(
        &self,
        id: Uuid,
        revision: u64,
        actor: &str,
        _scope: &RequestScope,
    ) -> Result<()> {
        self.decide(id, |request| {
            Self::close(request, ApprovalState::Applied, actor, None, Utc::now());
            request.revision = Some(revision);
            Ok(())
        })
        .map(drop)
    }

    async fn bypassed(
        &self,
        bypass: &Bypass,
        content_hash: &str,
        _covering: &[&ApprovalPolicy],
        _scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        bypass.check()?;
        let reason = format!("bypassed: {}", bypass.reason.trim());
        for request in locked(&self.requests).iter_mut().filter(|request| {
            request.content_hash == content_hash
                && matches!(
                    request.state,
                    ApprovalState::Pending | ApprovalState::Approved
                )
        }) {
            Self::close(
                request,
                ApprovalState::Applied,
                actor,
                Some(&reason),
                Utc::now(),
            );
        }
        Ok(())
    }
}

/// The types of the events recorded, in order.
#[derive(Default)]
pub(crate) struct MemoryEvents(Mutex<Vec<String>>);

impl MemoryEvents {
    pub(crate) fn types(&self) -> Vec<String> {
        locked(&self.0).clone()
    }
}

#[async_trait]
impl EventRecorder for MemoryEvents {
    async fn record(&self, event: EventDraft, _scope: &RequestScope, _actor: &str) {
        locked(&self.0).push(event.event_type().as_str().to_owned());
    }
}

/// A gateway that is ready, runs nothing and accepts no configuration.
pub(crate) struct IdleGateway;

#[async_trait]
impl GatewayUseCases for IdleGateway {
    async fn validate(&self, _document: ConfigDocument) -> Result<ValidationReport> {
        Ok(ValidationReport::valid())
    }

    async fn prepare(
        &self,
        _context: CommandContext,
        _document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        Err(PanelError::unavailable(
            "the gateway takes no configuration",
        ))
    }

    async fn activate(
        &self,
        _context: CommandContext,
        _prepare_token: String,
        _expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        Err(PanelError::unavailable(
            "the gateway takes no configuration",
        ))
    }

    async fn abort(
        &self,
        _context: CommandContext,
        _prepare_token: String,
    ) -> Result<AbortOutcome> {
        Err(PanelError::unavailable(
            "the gateway takes no configuration",
        ))
    }

    async fn status(&self) -> Result<GatewayStatus> {
        Ok(GatewayStatus::new(true, None, None, None, 0, "memory", "1"))
    }
}
