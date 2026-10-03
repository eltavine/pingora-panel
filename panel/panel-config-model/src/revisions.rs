//! Revisions: every configuration applied or attempted, as the API shows them.

use chrono::{DateTime, Utc};
use panel_errors::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What became of a revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RevisionOutcome {
    /// Sent to the gateway; settled when the attempt ends.
    Applying,
    /// What the gateway runs.
    Active,
    /// Ran until a later revision replaced it.
    Superseded,
    /// Failed validation before reaching the gateway.
    Rejected,
    /// The gateway refused it or the attempt was interrupted.
    Failed,
}

impl RevisionOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applying => "applying",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "applying" => Self::Applying,
            "active" => Self::Active,
            "superseded" => Self::Superseded,
            "rejected" => Self::Rejected,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Revision {
    pub id: u64,
    /// The draft version it was taken from.
    pub draft_version: u64,
    pub language_version: u32,
    /// SHA-256 of its files.
    pub content_hash: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub outcome: RevisionOutcome,
    pub outcome_at: DateTime<Utc>,
    /// Why a rejected or failed revision did not run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<Object>))]
    pub diagnostics: Vec<Diagnostic>,
    /// The runtime snapshot compiled from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
    /// The revision number the gateway reports for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_revision: Option<u64>,
}

/// A page of revisions, newest first.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RevisionList {
    pub items: Vec<Revision>,
    /// Pass as `before` for the next page; absent on the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before: Option<u64>,
}

/// A revision with its files.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RevisionDetail {
    pub revision: Revision,
    /// The configuration's files by path.
    pub files: BTreeMap<String, String>,
}
