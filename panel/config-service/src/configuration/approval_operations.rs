//! Approval policies, decisions on requests, and the gate an apply passes
//! (ADR 0019).

use super::{json_output, ConfigurationService};
use crate::{
    approval_rules::{assess, Bypass, Gate, Opening},
    language, operations,
    store::{ChangeOutput, DraftState},
};
use chrono::{DateTime, Utc};
use panel_application::{CommandContext, ContentHash};
use panel_config_api::{ApplyRequest, ApprovalChange, ApprovalQuery};
use panel_config_model::{ApprovalPolicy, ApprovalRequest, ApprovalRequestList};
use panel_errors::Result;
use serde_json::{json, Value};
use uuid::Uuid;

const DEFAULT_APPROVAL_PAGE: u32 = 50;
const MAX_APPROVAL_PAGE: u32 = 200;

/// A request as of `now`, against the draft's content.
fn current(mut request: ApprovalRequest, now: DateTime<Utc>, draft_hash: &ContentHash) -> Value {
    request.state = request.state_at(now, draft_hash.as_str());
    serde_json::to_value(&request).expect("API values serialize")
}

impl ConfigurationService {
    /// Reads of approval policies and requests. Requests report their state
    /// as of now.
    pub(super) async fn read_approvals(
        &self,
        draft: &DraftState,
        query: ApprovalQuery,
    ) -> Result<operations::Output> {
        let now = Utc::now();
        let draft_hash = language::content_hash(&draft.sources);
        Ok(match query {
            ApprovalQuery::Policies => {
                json_output(&self.approvals.policies().await?, String::new())
            }
            ApprovalQuery::Policy { id } => {
                json_output(&self.approvals.policy(&id).await?, String::new())
            }
            ApprovalQuery::Requests { before, limit } => {
                let limit = limit
                    .unwrap_or(DEFAULT_APPROVAL_PAGE)
                    .clamp(1, MAX_APPROVAL_PAGE);
                let mut items = self.approvals.requests(before, limit).await?;
                for item in &mut items {
                    item.state = item.state_at(now, draft_hash.as_str());
                }
                let next_before = (items.len() == limit as usize)
                    .then(|| items.last().map(|last| last.requested_at))
                    .flatten();
                json_output(&ApprovalRequestList { items, next_before }, String::new())
            }
            ApprovalQuery::Request { id } => json_output(
                &current(self.approvals.request(id).await?, now, &draft_hash),
                String::new(),
            ),
        })
    }

    /// The time and the draft content a decision on a request is made against.
    async fn decision_basis(&self) -> Result<(DateTime<Utc>, ContentHash)> {
        let draft = self.drafts.load().await?;
        Ok((Utc::now(), language::content_hash(&draft.sources)))
    }

    /// Changes to approval policies and decisions on requests.
    pub(super) async fn change_approvals(
        &self,
        context: &CommandContext,
        change: ApprovalChange,
    ) -> Result<ChangeOutput> {
        let scope = context.scope();
        let actor = context.actor();
        let value = match change {
            ApprovalChange::PutPolicy { id, policy } => {
                let (policy, created) = self
                    .approvals
                    .put_policy(&id, policy, &scope, actor)
                    .await?;
                json!({ "policy": policy, "created": created })
            }
            ApprovalChange::DeletePolicy { id } => {
                self.approvals.delete_policy(&id, &scope, actor).await?;
                json!({})
            }
            ApprovalChange::Approve { id } => {
                let (now, hash) = self.decision_basis().await?;
                let request = self
                    .approvals
                    .approve(id, actor, hash.as_str(), now, &scope)
                    .await?;
                current(request, now, &hash)
            }
            ApprovalChange::Reject { id, reason } => {
                let (now, hash) = self.decision_basis().await?;
                let reason = reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty());
                let request = self
                    .approvals
                    .reject(id, actor, reason, hash.as_str(), now, &scope)
                    .await?;
                current(request, now, &hash)
            }
            ApprovalChange::Withdraw { id } => {
                let (now, hash) = self.decision_basis().await?;
                current(
                    self.approvals.withdraw(id, actor, now, &scope).await?,
                    now,
                    &hash,
                )
            }
            ApprovalChange::Revoke { id } => {
                let (now, hash) = self.decision_basis().await?;
                current(
                    self.approvals.revoke(id, actor, now, &scope).await?,
                    now,
                    &hash,
                )
            }
        };
        Ok(ChangeOutput {
            content: serde_json::to_vec(&value).expect("API values serialize"),
            etag: String::new(),
        })
    }

    /// Whether policies let the draft through: no policy covers it, enough
    /// people approved it, or an Administrator bypassed them. Otherwise the
    /// request it waits on.
    pub(super) async fn approval_gate(
        &self,
        context: &CommandContext,
        request: &ApplyRequest,
        draft: &DraftState,
        content_hash: &str,
        note: Option<&str>,
    ) -> Result<std::result::Result<Option<Uuid>, Box<ApprovalRequest>>> {
        let policies = self.approvals.policies().await?;
        if !policies.iter().any(|policy| policy.policy.enabled) {
            return Ok(Ok(None));
        }
        let (active, _) = self.active().await?;
        let assessment = assess(&active, &draft.model);
        let now = Utc::now();
        let covering: Vec<&ApprovalPolicy> = policies
            .iter()
            .filter(|policy| policy.covers(&assessment, now))
            .collect();
        if covering.is_empty() {
            return Ok(Ok(None));
        }
        let scope = context.scope();
        if let Some(bypass) = &request.bypass {
            let bypass = Bypass {
                reason: bypass.reason.clone(),
                incident: bypass.incident.clone(),
            };
            self.approvals
                .bypassed(&bypass, content_hash, &covering, &scope, context.actor())
                .await?;
            return Ok(Ok(None));
        }
        let opening = Opening {
            draft_version: draft.version,
            content_hash,
            note,
            assessment,
            covering,
        };
        Ok(
            match self
                .approvals
                .gate(opening, now, &scope, context.actor())
                .await?
            {
                Gate::Clear => Ok(None),
                Gate::Approved(id) => Ok(Some(id)),
                Gate::Awaiting(waiting) => Err(waiting),
            },
        )
    }
}
