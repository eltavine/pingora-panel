//! What `ops-agent` does on the host for the panel (ADR 0028, ADR 0030).

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{sync::Arc, time::SystemTime};

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

/// A process that holds a listening socket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListeningProcess {
    pub pid: i32,
    pub name: String,
    /// Empty when the agent may not read it.
    pub executable: String,
    pub uid: u32,
}

/// A socket listening on a TCP port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortListener {
    pub address: String,
    pub port: u16,
    /// The socket's owner.
    pub uid: u32,
    pub processes: Vec<ListeningProcess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListenersReport {
    pub observed_at: Option<SystemTime>,
    pub listeners: Vec<PortListener>,
}

/// What the agent may do to the gateway's unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnitAction {
    Start,
    Stop,
    Restart,
}

impl UnitAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// The gateway's systemd unit in systemd's own words, such as `active` or
/// `failed`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GatewayUnitStatus {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    /// 0 when there is none.
    pub main_pid: u32,
    pub active_since: Option<SystemTime>,
    pub restarts: u32,
    pub result: String,
}

#[async_trait]
pub trait HostAgentPort: Send + Sync {
    /// The agent and its capabilities; fails as unsupported when no agent
    /// is configured and as unavailable when it does not answer.
    async fn agent(&self, scope: RequestScope) -> Result<AgentDescription>;

    async fn directories(&self, scope: RequestScope) -> Result<DirectoriesReport>;

    /// What listens on `ports`; 80 and 443 when empty.
    async fn listeners(&self, scope: RequestScope, ports: Vec<u16>) -> Result<ListenersReport>;

    async fn gateway_unit(&self, scope: RequestScope) -> Result<GatewayUnitStatus>;

    /// Starts, stops or restarts the gateway's unit and answers once
    /// systemd has finished.
    async fn change_gateway_unit(
        &self,
        context: CommandContext,
        action: UnitAction,
    ) -> Result<GatewayUnitStatus>;
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

    async fn listeners(&self, _: RequestScope, _: Vec<u16>) -> Result<ListenersReport> {
        Err(Self::refusal())
    }

    async fn gateway_unit(&self, _: RequestScope) -> Result<GatewayUnitStatus> {
        Err(Self::refusal())
    }

    async fn change_gateway_unit(
        &self,
        _: CommandContext,
        _: UnitAction,
    ) -> Result<GatewayUnitStatus> {
        Err(Self::refusal())
    }
}

/// A host agent port that records each change to the gateway's unit,
/// refused or not.
pub struct RecordedHostAgent {
    inner: Arc<dyn HostAgentPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedHostAgent {
    pub fn new(inner: Arc<dyn HostAgentPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl HostAgentPort for RecordedHostAgent {
    async fn agent(&self, scope: RequestScope) -> Result<AgentDescription> {
        self.inner.agent(scope).await
    }

    async fn directories(&self, scope: RequestScope) -> Result<DirectoriesReport> {
        self.inner.directories(scope).await
    }

    async fn listeners(&self, scope: RequestScope, ports: Vec<u16>) -> Result<ListenersReport> {
        self.inner.listeners(scope, ports).await
    }

    async fn gateway_unit(&self, scope: RequestScope) -> Result<GatewayUnitStatus> {
        self.inner.gateway_unit(scope).await
    }

    async fn change_gateway_unit(
        &self,
        context: CommandContext,
        action: UnitAction,
    ) -> Result<GatewayUnitStatus> {
        let result = self
            .inner
            .change_gateway_unit(context.clone(), action)
            .await;
        let operation = Operation::GatewayUnit {
            action,
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use panel_errors::ErrorCode;
    use std::sync::Mutex;

    /// Fails every change to a unit named `denied.service`.
    struct Unit;

    #[async_trait]
    impl HostAgentPort for Unit {
        async fn agent(&self, _: RequestScope) -> Result<AgentDescription> {
            Ok(AgentDescription::default())
        }

        async fn directories(&self, _: RequestScope) -> Result<DirectoriesReport> {
            Err(NoHostAgent::refusal())
        }

        async fn listeners(&self, _: RequestScope, _: Vec<u16>) -> Result<ListenersReport> {
            Err(NoHostAgent::refusal())
        }

        async fn gateway_unit(&self, _: RequestScope) -> Result<GatewayUnitStatus> {
            Ok(GatewayUnitStatus::default())
        }

        async fn change_gateway_unit(
            &self,
            context: CommandContext,
            action: UnitAction,
        ) -> Result<GatewayUnitStatus> {
            if context.actor() == "intruder" {
                return Err(PanelError::precondition_failed("polkit said no"));
            }
            Ok(GatewayUnitStatus {
                name: "pingora-panel-gatewayd.service".into(),
                active_state: if action == UnitAction::Stop {
                    "inactive"
                } else {
                    "active"
                }
                .into(),
                ..GatewayUnitStatus::default()
            })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, std::result::Result<String, String>)>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::GatewayUnit { action, result } = operation {
                self.0.lock().unwrap().push((
                    action.as_str().to_owned(),
                    result
                        .map(|status| status.active_state.clone())
                        .map_err(|error| error.code.as_str().to_owned()),
                ));
            }
        }
    }

    fn context(actor: &str) -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            actor,
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn every_change_to_the_unit_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let agent = RecordedHostAgent::new(Arc::new(Unit), recorder.clone());
        agent
            .change_gateway_unit(context("ops"), UnitAction::Stop)
            .await
            .unwrap();
        agent
            .change_gateway_unit(context("intruder"), UnitAction::Restart)
            .await
            .unwrap_err();
        agent
            .gateway_unit(RequestScope::new(RequestId::new("read").unwrap()))
            .await
            .unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                ("stop".to_owned(), Ok("inactive".to_owned())),
                (
                    "restart".to_owned(),
                    Err(ErrorCode::PRECONDITION_FAILED.to_owned())
                ),
            ]
        );
    }

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
