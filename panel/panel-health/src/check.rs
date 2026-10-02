use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Health of a service or component, ordered from healthy to failed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Healthy.
    Pass,
    /// Healthy, with concerns.
    Warn,
    /// Unhealthy.
    Fail,
}

/// Kind of the checked component, as named by the health response format.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentType {
    /// A service or library the service depends on.
    Component,
    /// A database or other persistent store.
    Datastore,
    /// A resource of the host, such as disk or memory.
    System,
}

/// What a failing check means for the service that registered it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Impact {
    /// The service cannot serve at all; readiness fails.
    Required,
    /// The service keeps serving reads but suspends writes.
    Degrading,
    /// The failure is reported but does not change what the service serves.
    Informational,
}

/// The result of one check. `output` is shown to operators and must not
/// carry secrets, addresses or raw driver messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckOutcome {
    status: HealthStatus,
    output: Option<String>,
}

impl CheckOutcome {
    pub fn pass() -> Self {
        Self {
            status: HealthStatus::Pass,
            output: None,
        }
    }

    pub fn warn(output: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Warn,
            output: Some(output.into()),
        }
    }

    pub fn fail(output: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Fail,
            output: Some(output.into()),
        }
    }

    pub fn status(&self) -> HealthStatus {
        self.status
    }

    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }
}

/// A dependency probe. Implementations should be cheap and must bound their
/// own work; the registry additionally enforces a timeout.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Stable component name, such as `postgresql`.
    fn component(&self) -> &str;

    fn component_type(&self) -> ComponentType;

    async fn check(&self) -> CheckOutcome;
}
