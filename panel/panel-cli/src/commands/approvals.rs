//! Approval policies and the requests applying covered changes opens.

use crate::{
    client::{Api, Result},
    output::{text, Column, Output},
};
use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum ApprovalCommand {
    /// Requests, newest first, with their state now.
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One request: its changes, policies and approvals.
    Show { id: String },
    /// Approves a request someone else opened.
    Approve { id: String },
    /// Rejects a request someone else opened.
    Reject {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Takes back your approval of a request not yet applied.
    Revoke { id: String },
    /// Withdraws your own request.
    Withdraw { id: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Risk {
    Low,
    High,
}

pub(crate) fn window(value: &str) -> std::result::Result<Value, String> {
    let (days, span) = match value.trim().rsplit_once(' ') {
        Some((days, span)) => (days.trim(), span),
        None => ("", value.trim()),
    };
    let (start, end) = span
        .split_once('-')
        .ok_or("expected [DAYS ]HH:MM-HH:MM, such as \"mon,tue 09:00-18:00\"")?;
    let days: Vec<&str> = days
        .split(',')
        .map(str::trim)
        .filter(|day| !day.is_empty())
        .collect();
    Ok(json!({"days": days, "start": start, "end": end}))
}

#[derive(Args)]
pub(crate) struct SetPolicy {
    id: String,
    #[arg(long, default_value = "")]
    description: String,
    /// Covers changes to resources of this kind, such as sites or
    /// listeners; any kind without it.
    #[arg(long = "resource")]
    resources: Vec<String>,
    /// Covers changes to sites with this tag; any change without it.
    #[arg(long = "site-tag")]
    site_tags: Vec<String>,
    /// Covers changes at least this risky.
    #[arg(long, value_enum, default_value_t = Risk::Low)]
    min_risk: Risk,
    /// Covers changes applied in this UTC window, as
    /// "[DAYS ]HH:MM-HH:MM"; always without it.
    #[arg(long = "window", value_parser = window)]
    windows: Vec<Value>,
    /// How many people other than the requester must approve.
    #[arg(long, default_value_t = 1)]
    approvals: u32,
    /// How long an approval stays valid.
    #[arg(long, default_value_t = 60)]
    valid_minutes: u32,
    /// Keep the policy but stop it from covering anything.
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
pub(crate) enum ApprovalPolicyCommand {
    /// Every approval policy.
    List,
    /// One approval policy.
    Show { id: String },
    /// Creates or replaces an approval policy; approvals given under the
    /// old version stop counting.
    Set(Box<SetPolicy>),
    /// Deletes an approval policy.
    Delete { id: String },
}

fn approvals(request: &Value) -> String {
    let valid = request["approvals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|approval| approval["revoked_at"].is_null())
        .count();
    format!("{valid}/{}", text(&request["required"]))
}

fn policies(request: &Value) -> String {
    let named: Vec<String> = request["policies"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|policy| format!("{}@v{}", text(&policy["id"]), text(&policy["version"])))
        .collect();
    if named.is_empty() {
        "-".into()
    } else {
        named.join(", ")
    }
}

const REQUESTS: &[Column] = &[
    ("ID", |request| text(&request["id"])),
    ("STATE", |request| text(&request["state"])),
    ("REQUESTED BY", |request| text(&request["requested_by"])),
    ("RISK", |request| text(&request["risk"])),
    ("APPROVALS", approvals),
    ("POLICIES", policies),
    ("REQUESTED", |request| text(&request["requested_at"])),
];

const REQUEST: &[Column] = &[
    ("ID", |request| text(&request["id"])),
    ("State", |request| text(&request["state"])),
    ("Requested by", |request| text(&request["requested_by"])),
    ("Requested", |request| text(&request["requested_at"])),
    ("Expires", |request| text(&request["expires_at"])),
    ("Draft version", |request| text(&request["draft_version"])),
    ("Risk", |request| text(&request["risk"])),
    ("Policies", policies),
    ("Approvals", approvals),
    ("Approved by", |request| {
        let names: Vec<String> = request["approvals"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|approval| {
                if approval["revoked_at"].is_null() {
                    format!(
                        "{} until {}",
                        text(&approval["approver"]),
                        text(&approval["valid_until"])
                    )
                } else {
                    format!("{} (revoked)", text(&approval["approver"]))
                }
            })
            .collect();
        if names.is_empty() {
            "-".into()
        } else {
            names.join(", ")
        }
    }),
    ("Changes", |request| {
        let changes: Vec<String> = request["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|change| format!("{} {}", text(&change["change"]), text(&change["resource"])))
            .collect();
        changes.join(", ")
    }),
    ("Note", |request| text(&request["note"])),
    ("Closed by", |request| text(&request["closed_by"])),
    ("Reason", |request| text(&request["reason"])),
    ("Revision", |request| text(&request["revision"])),
];

fn covers(policy: &Value) -> String {
    let list = |key: &str| -> Vec<String> {
        policy[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect()
    };
    let mut parts = Vec::new();
    let resources = list("resources");
    parts.push(if resources.is_empty() {
        "any change".to_owned()
    } else {
        resources.join("/")
    });
    let tags = list("site_tags");
    if !tags.is_empty() {
        parts.push(format!("sites tagged {}", tags.join(" or ")));
    }
    if policy["min_risk"] == "high" {
        parts.push("high risk".into());
    }
    for window in policy["windows"].as_array().into_iter().flatten() {
        let days: Vec<String> = window["days"]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        parts.push(format!(
            "{}{}-{} UTC",
            if days.is_empty() {
                String::new()
            } else {
                format!("{} ", days.join(","))
            },
            text(&window["start"]),
            text(&window["end"])
        ));
    }
    parts.join(", ")
}

const POLICIES: &[Column] = &[
    ("ID", |policy| text(&policy["id"])),
    ("STATE", |policy| {
        if policy["enabled"] == false {
            "disabled".into()
        } else {
            "enabled".into()
        }
    }),
    ("COVERS", covers),
    ("APPROVALS", |policy| text(&policy["approvals"])),
    ("VALID MINUTES", |policy| text(&policy["valid_minutes"])),
    ("VERSION", |policy| text(&policy["version"])),
];

const POLICY: &[Column] = &[
    ("ID", |policy| text(&policy["id"])),
    ("Description", |policy| text(&policy["description"])),
    ("Covers", covers),
    ("Approvals", |policy| text(&policy["approvals"])),
    ("Valid minutes", |policy| text(&policy["valid_minutes"])),
    ("Enabled", |policy| text(&policy["enabled"])),
    ("Version", |policy| text(&policy["version"])),
    ("Updated", |policy| text(&policy["updated_at"])),
];

/// What `ppanel config apply` reports when the change waits for approval.
pub(crate) fn waiting_message(request: &Value) -> String {
    format!(
        "Waiting for approval: request {} needs {} approval(s) from someone else under {}; \
         once approved with `ppanel approval approve {}`, apply again",
        text(&request["id"]),
        text(&request["required"]),
        policies(request),
        text(&request["id"]),
    )
}

pub(crate) async fn approval(api: &Api, output: &Output, command: ApprovalCommand) -> Result<()> {
    let decide = |id: &str, action: &str| format!("/api/v1/approvals/{id}/{action}");
    let (request, done) = match command {
        ApprovalCommand::List { limit } => {
            let page = api
                .get("/api/v1/approvals", &[("limit", limit.to_string())])
                .await?
                .body;
            output.list(&page["items"], REQUESTS);
            return Ok(());
        }
        ApprovalCommand::Show { id } => {
            let request = api.get(&format!("/api/v1/approvals/{id}"), &[]).await?.body;
            output.item(&request, REQUEST);
            return Ok(());
        }
        ApprovalCommand::Approve { id } => (
            api.change(Method::POST, &decide(&id, "approve"), None, None)
                .await?
                .body,
            "Approved",
        ),
        ApprovalCommand::Reject { id, reason } => (
            api.change(
                Method::POST,
                &decide(&id, "reject"),
                Some(&json!({ "reason": reason })),
                None,
            )
            .await?
            .body,
            "Rejected",
        ),
        ApprovalCommand::Revoke { id } => (
            api.change(Method::POST, &decide(&id, "revoke"), None, None)
                .await?
                .body,
            "Revoked your approval of",
        ),
        ApprovalCommand::Withdraw { id } => (
            api.change(Method::POST, &decide(&id, "withdraw"), None, None)
                .await?
                .body,
            "Withdrew",
        ),
    };
    output.done(
        &format!(
            "{done} request {}; it is now {}",
            text(&request["id"]),
            text(&request["state"])
        ),
        &request,
    );
    Ok(())
}

pub(crate) async fn approval_policy(
    api: &Api,
    output: &Output,
    command: ApprovalPolicyCommand,
) -> Result<()> {
    match command {
        ApprovalPolicyCommand::List => {
            let policies = api.get("/api/v1/approval-policies", &[]).await?.body;
            output.list(&policies, POLICIES);
        }
        ApprovalPolicyCommand::Show { id } => {
            let policy = api
                .get(&format!("/api/v1/approval-policies/{id}"), &[])
                .await?
                .body;
            output.item(&policy, POLICY);
        }
        ApprovalPolicyCommand::Set(policy) => {
            let body = json!({
                "description": policy.description,
                "resources": policy.resources,
                "site_tags": policy.site_tags,
                "min_risk": match policy.min_risk {
                    Risk::Low => "low",
                    Risk::High => "high",
                },
                "windows": policy.windows,
                "approvals": policy.approvals,
                "valid_minutes": policy.valid_minutes,
                "enabled": !policy.disabled,
            });
            let saved = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/approval-policies/{}", policy.id),
                    Some(&body),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Saved the approval policy {} as version {}",
                    policy.id,
                    text(&saved["version"])
                ),
                &saved,
            );
        }
        ApprovalPolicyCommand::Delete { id } => {
            api.change(
                Method::DELETE,
                &format!("/api/v1/approval-policies/{id}"),
                None,
                None,
            )
            .await?;
            output.done(&format!("Deleted the approval policy {id}"), &Value::Null);
        }
    }
    Ok(())
}
