//! The rules of approvals (ADR 0019), apart from where approvals are kept:
//! what a change touches, which open requests still fit it, and who may
//! decide what. A store applies them within its own transaction.

use chrono::{DateTime, Duration, Utc};
use panel_config_dsl::plan::plan;
use panel_config_model::{
    lua_changes, ApprovalPolicy, ApprovalRequest, ApprovalState, Assessment, ConfigModel,
    PlannedChange, PolicyVersion, REQUEST_LIFETIME,
};
use panel_errors::{PanelError, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

/// What a change touches, from the plan people review. A change to a
/// server's or route's Lua touches `lua` as well as the site, so policies on
/// Lua cover it.
pub fn assess(current: &ConfigModel, next: &ConfigModel) -> Assessment {
    let mut changes: Vec<PlannedChange> = plan(current, next)
        .into_iter()
        .map(|change| PlannedChange {
            resource: change.resource,
            change: serde_json::to_value(change.change)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect();
    let lua = lua_changes(current, next);
    if !lua.sites.is_empty()
        && !changes
            .iter()
            .any(|change| change.resource.split('/').next() == Some("lua"))
    {
        changes.push(PlannedChange {
            resource: "lua".into(),
            change: "changed".into(),
        });
    }
    let touched: BTreeSet<&str> = changes
        .iter()
        .filter_map(|change| change.resource.strip_prefix("sites/"))
        .collect();
    let site_tags = [current, next]
        .iter()
        .flat_map(|model| model.sites.iter())
        .filter(|site| touched.contains(site.id.to_string().as_str()))
        .flat_map(|site| site.tags.iter().cloned())
        .collect();
    Assessment::new(changes, site_tags)
}

/// What the approvals say about applying a change.
pub enum Gate {
    /// No policy covers it.
    Clear,
    /// Enough people approved exactly this content under these policies.
    Approved(Uuid),
    /// It waits for approvals.
    Awaiting(Box<ApprovalRequest>),
}

/// A change to open a request for.
pub struct Opening<'a> {
    pub draft_version: u64,
    pub content_hash: &'a str,
    pub note: Option<&'a str>,
    pub assessment: Assessment,
    pub covering: Vec<&'a ApprovalPolicy>,
}

impl Opening<'_> {
    /// The versions of the covering policies, which a request is bound to.
    fn versions(&self) -> Vec<PolicyVersion> {
        let mut versions: Vec<PolicyVersion> = self
            .covering
            .iter()
            .map(|policy| PolicyVersion {
                id: policy.id.clone(),
                version: policy.version,
            })
            .collect();
        versions.sort();
        versions
    }
}

/// Why an Administrator applied without the approvals.
#[derive(Clone, Debug, Serialize)]
pub struct Bypass {
    pub reason: String,
    pub incident: String,
}

impl Bypass {
    pub fn check(&self) -> Result<()> {
        if self.reason.trim().chars().count() < 10 || self.incident.trim().is_empty() {
            return Err(PanelError::invalid_argument(
                "an emergency bypass needs a reason of at least 10 characters and an incident",
            ));
        }
        Ok(())
    }
}

/// How the open requests for a change's content bear on applying it.
pub struct Screening {
    /// What the request that still fits says, if one does.
    pub gate: Option<Gate>,
    /// The requests that no longer fit, which close as outdated.
    pub outdated: Vec<Uuid>,
}

/// Screens the open requests for the opening's content, newest first: the
/// first that fits its policies decides; those before it no longer apply.
pub fn screen(opening: &Opening<'_>, open: Vec<ApprovalRequest>, now: DateTime<Utc>) -> Screening {
    let versions = opening.versions();
    let mut outdated = Vec::new();
    for existing in open {
        let current = existing.policies == versions;
        match existing.state_at(now, opening.content_hash) {
            ApprovalState::Approved if current => {
                return Screening {
                    gate: Some(Gate::Approved(existing.id)),
                    outdated,
                }
            }
            ApprovalState::Pending if current => {
                return Screening {
                    gate: Some(Gate::Awaiting(Box::new(existing))),
                    outdated,
                }
            }
            _ => outdated.push(existing.id),
        }
    }
    Screening {
        gate: None,
        outdated,
    }
}

/// The request applying the opening's content opens when none fits: as many
/// approvals as the strictest policy asks, valid as long as the shortest
/// allows.
pub fn open_request(opening: Opening<'_>, actor: &str, now: DateTime<Utc>) -> ApprovalRequest {
    let policies = opening.versions();
    let required = opening
        .covering
        .iter()
        .map(|policy| policy.policy.approvals)
        .max()
        .unwrap_or(1);
    let valid_minutes = opening
        .covering
        .iter()
        .map(|policy| policy.policy.valid_minutes)
        .min()
        .unwrap_or(60);
    ApprovalRequest {
        id: Uuid::now_v7(),
        state: ApprovalState::Pending,
        draft_version: opening.draft_version,
        content_hash: opening.content_hash.to_owned(),
        requested_by: actor.to_owned(),
        requested_at: now,
        expires_at: now + REQUEST_LIFETIME,
        note: opening.note.map(str::to_owned),
        risk: opening.assessment.risk,
        policies,
        required,
        valid_minutes,
        changes: opening.assessment.changes,
        approvals: Vec::new(),
        closed_by: None,
        closed_at: None,
        reason: None,
        revision: None,
    }
}

fn open_at(request: &ApprovalRequest, now: DateTime<Utc>, draft_hash: &str) -> Result<()> {
    match request.state_at(now, draft_hash) {
        ApprovalState::Pending | ApprovalState::Approved => Ok(()),
        state => Err(PanelError::precondition_failed(format!(
            "the request is {}, so it can no longer be decided",
            serde_json::to_value(state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
        ))),
    }
}

fn still_open(request: &ApprovalRequest) -> Result<()> {
    if matches!(
        request.state,
        ApprovalState::Pending | ApprovalState::Approved
    ) {
        Ok(())
    } else {
        Err(PanelError::precondition_failed(
            "the request is already closed",
        ))
    }
}

/// An approval someone may give.
pub struct Approval {
    pub valid_until: DateTime<Utc>,
    /// Whether it gives the request all the approvals it needs.
    pub completes: bool,
}

/// `approver`'s approval of an open request for content the draft still
/// has; nobody approves their own request, or twice.
pub fn approval(
    request: &ApprovalRequest,
    approver: &str,
    draft_hash: &str,
    now: DateTime<Utc>,
) -> Result<Approval> {
    open_at(request, now, draft_hash)?;
    if request.requested_by == approver {
        return Err(PanelError::permission_denied(
            "nobody may approve their own request",
        ));
    }
    if request
        .approvals
        .iter()
        .any(|approval| approval.approver == approver && approval.is_valid(now))
    {
        return Err(PanelError::conflict("you already approved this request"));
    }
    Ok(Approval {
        valid_until: now + Duration::minutes(i64::from(request.valid_minutes)),
        completes: request.valid_approvals(now) + 1 >= request.required as usize,
    })
}

/// Whether `approver` may reject the request: an open one someone else
/// asked for.
pub fn check_rejection(
    request: &ApprovalRequest,
    approver: &str,
    draft_hash: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    open_at(request, now, draft_hash)?;
    if request.requested_by == approver {
        return Err(PanelError::permission_denied(
            "withdraw your own request instead of rejecting it",
        ));
    }
    Ok(())
}

/// Whether `actor` may withdraw the request: only its requester, while it
/// is open.
pub fn check_withdrawal(request: &ApprovalRequest, actor: &str) -> Result<()> {
    if request.requested_by != actor {
        return Err(PanelError::permission_denied(
            "only the person who asked can withdraw a request",
        ));
    }
    still_open(request)
}

/// Whether taking back `approver`'s approval of an open request leaves it
/// short of the approvals it needs.
pub fn revocation(request: &ApprovalRequest, approver: &str, now: DateTime<Utc>) -> Result<bool> {
    still_open(request)?;
    if !request
        .approvals
        .iter()
        .any(|approval| approval.approver == approver && approval.revoked_at.is_none())
    {
        return Err(PanelError::not_found("you have not approved this request"));
    }
    let remaining = request
        .approvals
        .iter()
        .filter(|approval| approval.approver != approver && approval.is_valid(now))
        .count();
    Ok(remaining < request.required as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_config_model::ApprovalDecision;
    use panel_errors::ErrorCode;

    fn policy(id: &str, approvals: u32, valid_minutes: u32) -> ApprovalPolicy {
        ApprovalPolicy {
            id: id.into(),
            policy: serde_json::from_value(serde_json::json!({
                "approvals": approvals,
                "valid_minutes": valid_minutes,
            }))
            .unwrap(),
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn opening<'a>(covering: Vec<&'a ApprovalPolicy>) -> Opening<'a> {
        Opening {
            draft_version: 3,
            content_hash: "content",
            note: None,
            assessment: Assessment::default(),
            covering,
        }
    }

    fn approved(approver: &str, now: DateTime<Utc>) -> ApprovalDecision {
        ApprovalDecision {
            approver: approver.into(),
            approved_at: now,
            valid_until: now + Duration::minutes(30),
            revoked_at: None,
        }
    }

    fn code(error: PanelError) -> String {
        error.code.as_str().to_owned()
    }

    #[test]
    fn lua_in_a_site_touches_lua_as_well() {
        let site: panel_config_model::Site = serde_json::from_value(serde_json::json!({
            "id": Uuid::now_v7(),
            "name": "shop",
            "action": {"type": "respond"},
            "created_at": "2026-10-03T00:00:00Z",
            "updated_at": "2026-10-03T00:00:00Z",
        }))
        .unwrap();
        let current = ConfigModel {
            sites: vec![site],
            ..ConfigModel::default()
        };
        let mut next = current.clone();
        next.sites[0].lua.log = Some(panel_config_model::LuaCode::inline("local n = 1"));
        let assessment = assess(&current, &next);
        assert!(assessment.kinds.contains("sites") && assessment.kinds.contains("lua"));
        assert_eq!(assessment.risk, panel_config_model::Risk::High);
        let mut file = current.clone();
        file.lua.files.insert("lua/a.lua".into(), "return 1".into());
        let assessment = assess(&current, &file);
        assert_eq!(
            assessment
                .changes
                .iter()
                .map(|change| change.resource.as_str())
                .collect::<Vec<_>>(),
            ["lua/a.lua"]
        );
        assert!(assessment.kinds.contains("lua"));
    }

    #[test]
    fn a_request_asks_the_strictest_policies_and_fits_only_their_versions() {
        let (lenient, strict) = (policy("lenient", 1, 120), policy("strict", 2, 30));
        let now = Utc::now();
        let request = open_request(opening(vec![&lenient, &strict]), "alice", now);
        assert_eq!((request.required, request.valid_minutes), (2, 30));
        assert_eq!(request.requested_by, "alice");

        let screening = screen(
            &opening(vec![&strict, &lenient]),
            vec![request.clone()],
            now,
        );
        assert!(matches!(screening.gate, Some(Gate::Awaiting(_))));
        assert!(screening.outdated.is_empty());

        let revised = ApprovalPolicy {
            version: 2,
            ..strict.clone()
        };
        let screening = screen(
            &opening(vec![&lenient, &revised]),
            vec![request.clone()],
            now,
        );
        assert!(screening.gate.is_none());
        assert_eq!(screening.outdated, [request.id]);
    }

    #[test]
    fn approvals_come_from_others_once_each_for_the_content_approved() {
        let strict = policy("strict", 2, 30);
        let now = Utc::now();
        let mut request = open_request(opening(vec![&strict]), "alice", now);
        let own = approval(&request, "alice", "content", now).err().unwrap();
        assert_eq!(code(own), ErrorCode::PERMISSION_DENIED);
        assert!(!approval(&request, "bob", "content", now).unwrap().completes);

        request.approvals.push(approved("bob", now));
        let twice = approval(&request, "bob", "content", now).err().unwrap();
        assert_eq!(code(twice), ErrorCode::CONFLICT);
        assert!(
            approval(&request, "carol", "content", now)
                .unwrap()
                .completes
        );
        let changed = approval(&request, "carol", "other content", now)
            .err()
            .unwrap();
        assert_eq!(code(changed), ErrorCode::PRECONDITION_FAILED);
    }

    #[test]
    fn decisions_belong_to_those_who_may_make_them() {
        let lenient = policy("lenient", 1, 30);
        let now = Utc::now();
        let mut request = open_request(opening(vec![&lenient]), "alice", now);
        let unknown = revocation(&request, "bob", now).err().unwrap();
        assert_eq!(code(unknown), ErrorCode::NOT_FOUND);

        request.approvals.push(approved("bob", now));
        request.state = ApprovalState::Approved;
        assert!(revocation(&request, "bob", now).unwrap());
        assert!(check_rejection(&request, "alice", "content", now).is_err());
        assert!(check_rejection(&request, "bob", "content", now).is_ok());
        assert!(check_withdrawal(&request, "bob").is_err());
        assert!(check_withdrawal(&request, "alice").is_ok());

        request.state = ApprovalState::Withdrawn;
        let closed = check_withdrawal(&request, "alice").err().unwrap();
        assert_eq!(code(closed), ErrorCode::PRECONDITION_FAILED);
    }

    #[test]
    fn a_bypass_gives_its_reason_and_incident() {
        let bypass = |reason: &str, incident: &str| Bypass {
            reason: reason.into(),
            incident: incident.into(),
        };
        assert!(bypass("too short", "INC-1").check().is_err());
        assert!(bypass("the gateway is down", " ").check().is_err());
        assert!(bypass("the gateway is down", "INC-1").check().is_ok());
    }
}
