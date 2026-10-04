//! What `ops-agent` does on the host for the panel (ADR 0028, ADR 0030).

use crate::RequestScope;
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::time::SystemTime;

/// Something the agent can do, served only while it can.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCapability {
    Directories,
    Listeners,
    GatewayUnit,
    Containers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityState {
    Available,
    NotEnabled,
    Unsupported,
    Denied,
    Unreachable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityStatus {
    pub capability: AgentCapability,
    pub state: CapabilityState,
    /// What to do about a state other than available.
    pub detail: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentDescription {
    pub build: String,
    pub protocol: String,
    pub hostname: String,
    pub capabilities: Vec<CapabilityStatus>,
}

impl AgentDescription {
    pub fn has(&self, capability: AgentCapability) -> bool {
        self.capabilities.iter().any(|status| {
            status.capability == capability && status.state == CapabilityState::Available
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryKind {
    Configuration,
    Logs,
    Certificates,
}

/// The space one of the panel's directories takes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryUsage {
    pub kind: DirectoryKind,
    pub path: String,
    /// False when the directory does not exist.
    pub present: bool,
    pub bytes: u64,
    pub files: u64,
    /// Entries that could not be read, and so are not counted.
    pub unreadable: u64,
    /// True when the counts are partial.
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoriesReport {
    pub observed_at: Option<SystemTime>,
    pub directories: Vec<DirectoryUsage>,
}

#[async_trait]
pub trait HostAgentPort: Send + Sync {
    /// The agent and its capabilities; fails as unsupported when no agent
    /// is configured and as unavailable when it does not answer.
    async fn agent(&self, scope: RequestScope) -> Result<AgentDescription>;

    async fn directories(&self, scope: RequestScope) -> Result<DirectoriesReport>;
}

/// The port of an installation without the agent.
pub struct NoHostAgent;

impl NoHostAgent {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("no host agent is configured")
    }
}

#[async_trait]
impl HostAgentPort for NoHostAgent {
    async fn agent(&self, _: RequestScope) -> Result<AgentDescription> {
        Err(Self::refusal())
    }

    async fn directories(&self, _: RequestScope) -> Result<DirectoriesReport> {
        Err(Self::refusal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;

    #[test]
    fn only_available_capabilities_count() {
        let status = |capability, state| CapabilityStatus {
            capability,
            state,
            detail: String::new(),
        };
        let description = AgentDescription {
            capabilities: vec![
                status(AgentCapability::Directories, CapabilityState::Available),
                status(AgentCapability::Listeners, CapabilityState::Denied),
            ],
            ..AgentDescription::default()
        };
        assert!(description.has(AgentCapability::Directories));
        assert!(!description.has(AgentCapability::Listeners));
        assert!(!description.has(AgentCapability::Containers));
    }

    #[tokio::test]
    async fn an_installation_without_the_agent_says_so() {
        let scope = RequestScope::new(panel_context::RequestId::new("request").unwrap());
        let refused = NoHostAgent.agent(scope).await.unwrap_err();
        assert_eq!(refused.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    }
}
