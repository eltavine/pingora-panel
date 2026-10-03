use crate::{ActivatedDeployment, CommandContext, RequestScope};
use async_trait::async_trait;
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

/// A named read of the configuration; `parameters` is a JSON object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationRead {
    pub operation: String,
    pub resource: String,
    pub parameters: Vec<u8>,
}

/// A named change; `content` is the JSON body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationChange {
    pub operation: String,
    pub resource: String,
    /// Entity tags the target must still match (RFC 9110 §13.1.1).
    pub if_match: Option<String>,
    pub content: Vec<u8>,
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
}

/// The configuration API: reads and changes of the draft and applying it.
#[async_trait]
pub trait ConfigurationPort: Send + Sync {
    async fn read(
        &self,
        scope: RequestScope,
        read: ConfigurationRead,
    ) -> Result<ConfigurationOutput>;

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput>;

    async fn apply(&self, context: CommandContext, request: ApplyRequest) -> Result<ApplyOutcome>;
}
