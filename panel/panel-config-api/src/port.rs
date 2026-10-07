use crate::{ConfigurationCommand, ConfigurationQuery};
use async_trait::async_trait;
use panel_application::{ActivatedDeployment, CommandContext, RequestScope};
use panel_config_model::ApprovalRequest;
use panel_errors::{Result, ValidationReport};
use std::time::SystemTime;

/// Version and application state of the draft configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DraftInfo {
    pub version: u64,
    pub updated_at: Option<SystemTime>,
    /// The draft version the gateway runs, if one was applied.
    pub applied_version: Option<u64>,
    pub applied_at: Option<SystemTime>,
}

impl DraftInfo {
    /// Whether the draft has changes the gateway does not run yet.
    pub fn pending(&self) -> bool {
        self.version > self.applied_version.unwrap_or(0)
    }
}

/// A change, and the entity tags its target must still match (RFC 9110
/// §13.1.1).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigurationChange {
    pub command: ConfigurationCommand,
    pub if_match: Option<String>,
}

impl ConfigurationChange {
    pub fn new(command: impl Into<ConfigurationCommand>) -> Self {
        Self {
            command: command.into(),
            if_match: None,
        }
    }

    pub fn if_match(mut self, tags: impl Into<String>) -> Self {
        self.if_match = Some(tags.into());
        self
    }
}

/// A JSON result with the draft state it was produced from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationOutput {
    pub content: Vec<u8>,
    pub etag: Option<String>,
    pub draft: DraftInfo,
}

/// What to apply and how.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApplyRequest {
    /// Refuses a draft that changed since this version; zero applies
    /// whatever is current.
    pub expected_version: u64,
    /// Recorded with the revision.
    pub note: Option<String>,
    /// Validates and prepares on the gateway without activating.
    pub dry_run: bool,
    /// Applies without the approvals policies ask for.
    pub bypass: Option<ApprovalBypass>,
    /// Refuses unless the plan is still the one with this digest, as the
    /// plan query reports it.
    pub expected_plan: Option<String>,
}

/// Why an approval was bypassed; only for actors allowed to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApprovalBypass {
    pub reason: String,
    /// The incident it answers, such as a ticket reference.
    pub incident: String,
}

impl ApprovalBypass {
    pub fn new(reason: impl Into<String>, incident: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            incident: incident.into(),
        }
    }
}

impl ApplyRequest {
    pub fn new(expected_version: u64) -> Self {
        Self {
            expected_version,
            ..Self::default()
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    pub fn bypassing(mut self, bypass: ApprovalBypass) -> Self {
        self.bypass = Some(bypass);
        self
    }

    pub fn expecting_plan(mut self, digest: impl Into<String>) -> Self {
        self.expected_plan = Some(digest.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ApplyOutcome {
    /// The gateway runs the configuration recorded as `revision`.
    #[non_exhaustive]
    Applied {
        draft: DraftInfo,
        deployment: ActivatedDeployment,
        revision: u64,
    },
    /// The draft failed validation and never reached the gateway; the
    /// attempt is recorded as `revision` when it got that far.
    #[non_exhaustive]
    Rejected {
        draft: DraftInfo,
        report: ValidationReport,
        revision: Option<u64>,
    },
    /// A dry run passed every check, the gateway's preparation included.
    #[non_exhaustive]
    Checked {
        draft: DraftInfo,
        report: ValidationReport,
    },
    /// Policies ask for approvals first; nothing was published.
    #[non_exhaustive]
    AwaitingApproval {
        draft: DraftInfo,
        request: Box<ApprovalRequest>,
    },
}

impl ApplyOutcome {
    pub fn applied(draft: DraftInfo, deployment: ActivatedDeployment, revision: u64) -> Self {
        Self::Applied {
            draft,
            deployment,
            revision,
        }
    }

    pub fn rejected(draft: DraftInfo, report: ValidationReport, revision: Option<u64>) -> Self {
        Self::Rejected {
            draft,
            report,
            revision,
        }
    }

    pub fn checked(draft: DraftInfo, report: ValidationReport) -> Self {
        Self::Checked { draft, report }
    }

    pub fn awaiting_approval(draft: DraftInfo, request: ApprovalRequest) -> Self {
        Self::AwaitingApproval {
            draft,
            request: Box::new(request),
        }
    }
}

/// The configuration API: reads and changes of the draft and applying it.
/// Results are JSON, as the HTTP API serves them.
#[async_trait]
pub trait ConfigurationPort: Send + Sync {
    async fn read(
        &self,
        scope: RequestScope,
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput>;

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput>;

    async fn apply(&self, context: CommandContext, request: ApplyRequest) -> Result<ApplyOutcome>;
}
