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

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ApplyOutcome {
    Applied {
        draft: DraftInfo,
        deployment: ActivatedDeployment,
    },
    /// The draft failed validation and never reached the gateway.
    Rejected {
        draft: DraftInfo,
        report: ValidationReport,
    },
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

    /// `expected_version` refuses to apply a draft that changed meanwhile;
    /// zero applies whatever is current.
    async fn apply(&self, context: CommandContext, expected_version: u64) -> Result<ApplyOutcome>;
}
