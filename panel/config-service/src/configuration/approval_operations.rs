//! Approval policies, decisions on requests, and the gate an apply passes
//! (ADR 0019).

use super::{decode, json_output, ConfigurationService};
use crate::{
    approvals::{assess, Bypass, Gate, Opening},
    draft::{ChangeOutput, DraftState},
    language, operations,
};
use chrono::{DateTime, Utc};
use panel_application::CommandContext;
use panel_config_model::{
    ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ApprovalRequestList,
};
use panel_contracts::config::v1 as wire;
use panel_errors::{PanelError, Result};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalPage {
    before: Option<DateTime<Utc>>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasonBody {
    reason: Option<String>,
}

const DEFAULT_APPROVAL_PAGE: u32 = 50;
const MAX_APPROVAL_PAGE: u32 = 200;

fn approval_id(resource: &str) -> Result<Uuid> {
    resource
        .strip_prefix("approvals/")
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| PanelError::not_found(format!("no resource {resource:?}")))
}

fn policy_id(resource: &str) -> Result<&str> {
    resource
        .strip_prefix("approval-policies/")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| PanelError::not_found(format!("no resource {resource:?}")))
}

impl ConfigurationService {
    /// Reads of approval policies and requests; `None` for every other
    /// operation. Requests report their state as of now.
    pub(super) async fn read_approvals(
        &self,
        draft: &DraftState,
        operation: &str,
        resource: &str,
        parameters: &[u8],
    ) -> Result<Option<operations::Output>> {
        let now = Utc::now();
        let draft_hash = language::content_hash(&draft.sources);
        let output = match (operation, resource) {
            ("approval_policies.list", "approval-policies") => {
                json_output(&self.approvals.policies().await?, String::new())
            }
            ("approval_policies.get", resource) if resource.starts_with("approval-policies/") => {
                json_output(
                    &self.approvals.policy(policy_id(resource)?).await?,
                    String::new(),
                )
            }
            ("approvals.list", "approvals") => {
                let page: ApprovalPage = decode(parameters)?;
                let limit = page
                    .limit
                    .unwrap_or(DEFAULT_APPROVAL_PAGE)
                    .clamp(1, MAX_APPROVAL_PAGE);
                let mut items = self.approvals.requests(page.before, limit).await?;
                for item in &mut items {
                    item.state = item.state_at(now, draft_hash.as_str());
                }
                let next_before = (items.len() == limit as usize)
                    .then(|| items.last().map(|last| last.requested_at))
                    .flatten();
                json_output(&ApprovalRequestList { items, next_before }, String::new())
            }
            ("approvals.get", resource) if resource.starts_with("approvals/") => {
                let mut item = self.approvals.request(approval_id(resource)?).await?;
                item.state = item.state_at(now, draft_hash.as_str());
                json_output(&item, String::new())
            }
            _ => return Ok(None),
        };
        Ok(Some(output))
    }

    /// Changes to approval policies and decisions on requests; `None` for
    /// every other operation.
    pub(super) async fn change_approvals(
        &self,
        context: &CommandContext,
        request: &wire::ChangeRequest,
    ) -> Result<Option<ChangeOutput>> {
        let scope = context.scope();
        let actor = context.actor();
        let resource = request.resource.as_str();
        let value = match request.operation.as_str() {
            "approval_policies.put" => {
                let input: ApprovalPolicyInput = decode(&request.content)?;
                let (policy, created) = self
                    .approvals
                    .put_policy(policy_id(resource)?, input, &scope, actor)
                    .await?;
                json!({ "policy": policy, "created": created })
            }
            "approval_policies.delete" => {
                self.approvals
                    .delete_policy(policy_id(resource)?, &scope, actor)
                    .await?;
                json!({})
            }
            operation @ ("approvals.approve" | "approvals.reject" | "approvals.withdraw"
            | "approvals.revoke") => {
                let id = approval_id(resource)?;
                let now = Utc::now();
                let draft = self.drafts.load().await?;
                let draft_hash = language::content_hash(&draft.sources);
                let mut decided = match operation {
                    "approvals.approve" => {
                        self.approvals
                            .approve(id, actor, draft_hash.as_str(), now, &scope)
                            .await?
                    }
                    "approvals.reject" => {
                        let body: ReasonBody = decode(&request.content)?;
                        let reason = body
                            .reason
                            .as_deref()
                            .map(str::trim)
                            .filter(|reason| !reason.is_empty());
                        self.approvals
                            .reject(id, actor, reason, draft_hash.as_str(), now, &scope)
                            .await?
                    }
                    "approvals.withdraw" => self.approvals.withdraw(id, actor, now, &scope).await?,
                    _ => self.approvals.revoke(id, actor, now, &scope).await?,
                };
                decided.state = decided.state_at(now, draft_hash.as_str());
                serde_json::to_value(&decided).expect("API values serialize")
            }
            _ => return Ok(None),
        };
        Ok(Some(ChangeOutput {
            content: serde_json::to_vec(&value).expect("API values serialize"),
            etag: String::new(),
        }))
    }

    /// Whether policies let the draft through: no policy covers it, enough
    /// people approved it, or an Administrator bypassed them. Otherwise the
    /// request it waits on.
    pub(super) async fn approval_gate(
        &self,
        context: &CommandContext,
        request: &wire::ApplyRequest,
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
        if !request.bypass_reason.is_empty() || !request.bypass_incident.is_empty() {
            let bypass = Bypass {
                reason: request.bypass_reason.clone(),
                incident: request.bypass_incident.clone(),
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
