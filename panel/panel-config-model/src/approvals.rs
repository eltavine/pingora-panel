//! Approvals of configuration changes (ADR 0019): the policies that decide
//! which changes need them, and the requests people approve.

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The most approvals a policy can ask for.
pub const MAX_APPROVALS: u32 = 5;
pub const MIN_VALID_MINUTES: u32 = 5;
/// A week.
pub const MAX_VALID_MINUTES: u32 = 7 * 24 * 60;
pub const DEFAULT_VALID_MINUTES: u32 = 60;
/// How long a request waits for decisions.
pub const REQUEST_LIFETIME: Duration = Duration::hours(24);
/// The kinds of resource a change can touch, as plans name them.
pub const RESOURCE_KINDS: &[&str] = &[
    "sites",
    "upstreams",
    "listeners",
    "tls-profiles",
    "security-policies",
];
/// Changes to these kinds are high-risk, as are removals.
const HIGH_RISK_KINDS: &[&str] = &["listeners", "tls-profiles", "security-policies"];
const MAX_ID: usize = 64;
const MAX_TEXT: usize = 256;

/// How much a change can break.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Risk {
    #[default]
    Low,
    /// Removes something, or touches listeners, TLS profiles or security
    /// policies.
    High,
}

/// A day of the week.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl From<Weekday> for Day {
    fn from(day: Weekday) -> Self {
        match day {
            Weekday::Mon => Self::Mon,
            Weekday::Tue => Self::Tue,
            Weekday::Wed => Self::Wed,
            Weekday::Thu => Self::Thu,
            Weekday::Fri => Self::Fri,
            Weekday::Sat => Self::Sat,
            Weekday::Sun => Self::Sun,
        }
    }
}

/// A span of time on some days, in UTC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TimeWindow {
    /// Every day when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<Day>,
    /// `HH:MM`, included.
    pub start: String,
    /// `HH:MM`, excluded; after `start`.
    pub end: String,
}

fn clock(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

impl TimeWindow {
    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        let (Some(start), Some(end)) = (clock(&self.start), clock(&self.end)) else {
            return false;
        };
        let time = at.time();
        (self.days.is_empty() || self.days.contains(&at.weekday().into()))
            && start <= time
            && time < end
    }

    fn problem(&self) -> Option<String> {
        match (clock(&self.start), clock(&self.end)) {
            (Some(start), Some(end)) if start < end => None,
            (Some(_), Some(_)) => Some(format!(
                "the window {}–{} must end after it starts; split one that crosses midnight",
                self.start, self.end
            )),
            _ => Some(format!(
                "the window {}–{} must use HH:MM times",
                self.start, self.end
            )),
        }
    }
}

const fn one() -> u32 {
    1
}

const fn default_valid_minutes() -> u32 {
    DEFAULT_VALID_MINUTES
}

const fn enabled() -> bool {
    true
}

/// An approval policy as an Administrator writes it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ApprovalPolicyInput {
    #[serde(default)]
    pub description: String,
    /// Covers changes to resources of these kinds; any kind when empty.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Covers changes to sites with one of these tags; any change when empty.
    #[serde(default)]
    pub site_tags: Vec<String>,
    /// Covers changes at least this risky.
    #[serde(default)]
    pub min_risk: Risk,
    /// Covers changes applied inside one of these windows; always when empty.
    #[serde(default)]
    pub windows: Vec<TimeWindow>,
    /// How many people other than the requester must approve.
    #[serde(default = "one")]
    pub approvals: u32,
    /// How long an approval stays valid.
    #[serde(default = "default_valid_minutes")]
    pub valid_minutes: u32,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

impl ApprovalPolicyInput {
    /// What is wrong with it; empty when nothing.
    pub fn problems(&self, id: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if id.is_empty()
            || id.len() > MAX_ID
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        {
            problems.push(format!(
                "the policy ID {id:?} must have 1 to {MAX_ID} letters, digits, dots, dashes or underscores"
            ));
        }
        if self.description.chars().count() > MAX_TEXT {
            problems.push(format!(
                "the description has more than {MAX_TEXT} characters"
            ));
        }
        for kind in &self.resources {
            if !RESOURCE_KINDS.contains(&kind.as_str()) {
                problems.push(format!(
                    "{kind:?} is not a resource kind; use one of {}",
                    RESOURCE_KINDS.join(", ")
                ));
            }
        }
        for tag in &self.site_tags {
            if tag.trim().is_empty() || tag.chars().count() > MAX_ID {
                problems.push(format!("the site tag {tag:?} is empty or too long"));
            }
        }
        problems.extend(self.windows.iter().filter_map(TimeWindow::problem));
        if !(1..=MAX_APPROVALS).contains(&self.approvals) {
            problems.push(format!("a policy asks for 1 to {MAX_APPROVALS} approvals"));
        }
        if !(MIN_VALID_MINUTES..=MAX_VALID_MINUTES).contains(&self.valid_minutes) {
            problems.push(format!(
                "approvals stay valid for {MIN_VALID_MINUTES} to {MAX_VALID_MINUTES} minutes"
            ));
        }
        problems
    }
}

/// An approval policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApprovalPolicy {
    pub id: String,
    #[serde(flatten)]
    pub policy: ApprovalPolicyInput,
    /// Increases with every change, which invalidates approvals given under
    /// an earlier version.
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ApprovalPolicy {
    /// Whether the policy asks for approval of a change applied at `at`.
    pub fn covers(&self, change: &Assessment, at: DateTime<Utc>) -> bool {
        let policy = &self.policy;
        let any = |wanted: &[String], present: &BTreeSet<String>| {
            wanted.is_empty() || wanted.iter().any(|item| present.contains(item))
        };
        policy.enabled
            && !change.kinds.is_empty()
            && any(&policy.resources, &change.kinds)
            && any(&policy.site_tags, &change.site_tags)
            && change.risk >= policy.min_risk
            && (policy.windows.is_empty()
                || policy.windows.iter().any(|window| window.contains(at)))
    }
}

/// One resource a change adds, changes or removes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlannedChange {
    /// Such as `sites/<id>`.
    pub resource: String,
    /// `added`, `changed` or `removed`.
    pub change: String,
}

/// What a change touches, as policies see it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Assessment {
    pub changes: Vec<PlannedChange>,
    pub kinds: BTreeSet<String>,
    /// The tags of every site the change touches, before and after.
    pub site_tags: BTreeSet<String>,
    pub risk: Risk,
}

impl Assessment {
    /// Classifies `changes`, with the tags of the sites they touch.
    pub fn new(changes: Vec<PlannedChange>, site_tags: BTreeSet<String>) -> Self {
        let kinds: BTreeSet<String> = changes
            .iter()
            .filter_map(|change| change.resource.split('/').next())
            .map(str::to_owned)
            .collect();
        let risky = changes.iter().any(|change| change.change == "removed")
            || HIGH_RISK_KINDS.iter().any(|kind| kinds.contains(*kind));
        Self {
            changes,
            kinds,
            site_tags,
            risk: if risky { Risk::High } else { Risk::Low },
        }
    }
}

/// A policy as it was when a request was opened.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PolicyVersion {
    pub id: String,
    pub version: u64,
}

/// Where a request stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApprovalState {
    /// Waiting for approvals.
    Pending,
    /// Enough valid approvals; applying the same content goes ahead.
    Approved,
    Rejected,
    /// Withdrawn by the requester.
    Withdrawn,
    /// Nobody decided in time, or the approvals ran out.
    Expired,
    /// The draft no longer has the content the request is about.
    Outdated,
    /// The approved content was applied.
    Applied,
}

/// One person's approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApprovalDecision {
    pub approver: String,
    pub approved_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApprovalDecision {
    pub fn is_valid(&self, at: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && at < self.valid_until
    }
}

/// A request to apply a change that policies cover.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApprovalRequest {
    pub id: Uuid,
    /// As of the time it was read.
    pub state: ApprovalState,
    pub draft_version: u64,
    /// The content approved; applying anything else needs a new request.
    pub content_hash: String,
    pub requested_by: String,
    pub requested_at: DateTime<Utc>,
    /// When it expires unless decided.
    pub expires_at: DateTime<Utc>,
    pub note: Option<String>,
    pub risk: Risk,
    pub policies: Vec<PolicyVersion>,
    /// Approvals needed from people other than the requester.
    pub required: u32,
    /// How long each approval stays valid.
    pub valid_minutes: u32,
    pub changes: Vec<PlannedChange>,
    pub approvals: Vec<ApprovalDecision>,
    /// Who rejected, withdrew or applied it, when and why.
    pub closed_by: Option<String>,
    pub closed_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    /// The revision it was applied as.
    pub revision: Option<u64>,
}

impl ApprovalRequest {
    /// Approvals that still count at `at`.
    pub fn valid_approvals(&self, at: DateTime<Utc>) -> usize {
        self.approvals
            .iter()
            .filter(|approval| approval.is_valid(at))
            .count()
    }

    /// Its state at `at` while the draft has `draft_hash`: an open request
    /// expires, runs out of approvals or falls behind the draft.
    pub fn state_at(&self, at: DateTime<Utc>, draft_hash: &str) -> ApprovalState {
        match self.state {
            ApprovalState::Pending | ApprovalState::Approved => {
                if self.content_hash != draft_hash {
                    ApprovalState::Outdated
                } else if self.valid_approvals(at) >= self.required as usize {
                    ApprovalState::Approved
                } else if at >= self.expires_at
                    || (self.state == ApprovalState::Approved && !self.approvals.is_empty())
                {
                    ApprovalState::Expired
                } else {
                    ApprovalState::Pending
                }
            }
            closed => closed,
        }
    }
}

/// A page of requests, newest first.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApprovalRequestList {
    pub items: Vec<ApprovalRequest>,
    /// Pass as `before` for the next page; absent on the last.
    pub next_before: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn policy(input: ApprovalPolicyInput) -> ApprovalPolicy {
        ApprovalPolicy {
            id: "p".into(),
            policy: input,
            version: 1,
            created_at: at("2026-10-01T00:00:00Z"),
            updated_at: at("2026-10-01T00:00:00Z"),
        }
    }

    fn input() -> ApprovalPolicyInput {
        serde_json::from_str("{}").unwrap()
    }

    fn change(resource: &str, change: &str) -> PlannedChange {
        PlannedChange {
            resource: resource.into(),
            change: change.into(),
        }
    }

    #[test]
    fn removals_and_edge_resources_are_risky() {
        let low = Assessment::new(vec![change("sites/a", "changed")], BTreeSet::new());
        assert_eq!(low.risk, Risk::Low);
        for risky in [
            change("sites/a", "removed"),
            change("listeners/edge", "changed"),
        ] {
            assert_eq!(
                Assessment::new(vec![risky], BTreeSet::new()).risk,
                Risk::High
            );
        }
    }

    #[test]
    fn every_condition_a_policy_sets_must_hold() {
        let monday_noon = at("2026-10-05T12:00:00Z");
        let tagged = Assessment::new(
            vec![change("sites/a", "changed")],
            BTreeSet::from(["production".to_owned()]),
        );
        assert!(policy(input()).covers(&tagged, monday_noon));
        let production = ApprovalPolicyInput {
            site_tags: vec!["production".into()],
            resources: vec!["sites".into()],
            ..input()
        };
        assert!(policy(production.clone()).covers(&tagged, monday_noon));
        let staging = Assessment::new(
            vec![change("sites/b", "changed")],
            BTreeSet::from(["staging".to_owned()]),
        );
        assert!(!policy(production).covers(&staging, monday_noon));
        let risky_only = ApprovalPolicyInput {
            min_risk: Risk::High,
            ..input()
        };
        assert!(!policy(risky_only).covers(&tagged, monday_noon));
        let office_hours = ApprovalPolicyInput {
            windows: vec![TimeWindow {
                days: vec![Day::Mon, Day::Tue],
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            ..input()
        };
        assert!(policy(office_hours.clone()).covers(&tagged, monday_noon));
        assert!(!policy(office_hours.clone()).covers(&tagged, at("2026-10-05T18:00:00Z")));
        assert!(!policy(office_hours).covers(&tagged, at("2026-10-07T12:00:00Z")));
        let off = ApprovalPolicyInput {
            enabled: false,
            ..input()
        };
        assert!(!policy(off).covers(&tagged, monday_noon));
        assert!(!policy(input()).covers(&Assessment::default(), monday_noon));
    }

    #[test]
    fn policies_are_checked_before_they_are_kept() {
        assert!(input().problems("prod").is_empty());
        let wrong = ApprovalPolicyInput {
            resources: vec!["routes".into()],
            windows: vec![TimeWindow {
                days: Vec::new(),
                start: "22:00".into(),
                end: "06:00".into(),
            }],
            approvals: 0,
            valid_minutes: 1,
            ..input()
        };
        assert_eq!(
            wrong.problems("a b").len(),
            5,
            "{:?}",
            wrong.problems("a b")
        );
    }

    #[test]
    fn requests_expire_run_out_and_fall_behind_the_draft() {
        let now = at("2026-10-05T12:00:00Z");
        let approval = |valid_until: &str| ApprovalDecision {
            approver: "bob".into(),
            approved_at: now,
            valid_until: at(valid_until),
            revoked_at: None,
        };
        let request = ApprovalRequest {
            id: Uuid::nil(),
            state: ApprovalState::Pending,
            draft_version: 3,
            content_hash: "h".into(),
            requested_by: "alice".into(),
            requested_at: now,
            expires_at: now + REQUEST_LIFETIME,
            note: None,
            risk: Risk::Low,
            policies: Vec::new(),
            required: 1,
            valid_minutes: 60,
            changes: Vec::new(),
            approvals: Vec::new(),
            closed_by: None,
            closed_at: None,
            reason: None,
            revision: None,
        };
        assert_eq!(request.state_at(now, "h"), ApprovalState::Pending);
        assert_eq!(request.state_at(now, "other"), ApprovalState::Outdated);
        assert_eq!(
            request.state_at(now + Duration::days(2), "h"),
            ApprovalState::Expired
        );
        let approved = ApprovalRequest {
            state: ApprovalState::Approved,
            approvals: vec![approval("2026-10-05T13:00:00Z")],
            ..request.clone()
        };
        assert_eq!(approved.state_at(now, "h"), ApprovalState::Approved);
        assert_eq!(
            approved.state_at(at("2026-10-05T13:00:00Z"), "h"),
            ApprovalState::Expired,
            "approvals stop counting when they run out"
        );
    }
}
