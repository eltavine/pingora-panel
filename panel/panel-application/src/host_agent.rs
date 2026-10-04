//! What `ops-agent` does on the host for the panel (ADR 0028, ADR 0030).

use crate::{CommandContext, ContainerSummary, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{sync::Arc, time::SystemTime};

/// Something the agent can do, served only while it can.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCapability {
    Directories,
    Listeners,
    GatewayService,
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

/// What the agent may do to the service that runs the gateway.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayServiceAction {
    Start,
    /// The container's stop signal, then SIGKILL once its stop timeout
    /// passes.
    Stop,
    Restart,
}

impl GatewayServiceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// The container that runs the gateway, as its engine reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayContainer {
    /// The engine it runs on, such as `docker`.
    pub engine: String,
    pub container: ContainerSummary,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    /// How it last stopped; 0 until it has.
    pub exit_code: i64,
    /// How often the engine restarted it under its restart policy.
    pub restarts: u32,
    /// `healthy`, `unhealthy` or `starting`; empty without a health check.
    pub health: String,
}

/// The service on the host that runs the gateway, and its state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayServiceStatus {
    pub observed_at: Option<SystemTime>,
    pub container: GatewayContainer,
}

#[async_trait]
pub trait HostAgentPort: Send + Sync {
    /// The agent and its capabilities; fails as unsupported when no agent
    /// is configured and as unavailable when it does not answer.
    async fn agent(&self, scope: RequestScope) -> Result<AgentDescription>;

    async fn directories(&self, scope: RequestScope) -> Result<DirectoriesReport>;

    /// What listens on `ports`; 80 and 443 when empty.
    async fn listeners(&self, scope: RequestScope, ports: Vec<u16>) -> Result<ListenersReport>;

    async fn gateway_service(&self, scope: RequestScope) -> Result<GatewayServiceStatus>;

    /// Starts, stops or restarts the service that runs the gateway and
    /// answers once its engine has finished.
    async fn change_gateway_service(
        &self,
        context: CommandContext,
        action: GatewayServiceAction,
    ) -> Result<GatewayServiceStatus>;
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

    async fn gateway_service(&self, _: RequestScope) -> Result<GatewayServiceStatus> {
        Err(Self::refusal())
    }

    async fn change_gateway_service(
        &self,
        _: CommandContext,
        _: GatewayServiceAction,
    ) -> Result<GatewayServiceStatus> {
        Err(Self::refusal())
    }
}

/// A host agent port that records each change to the gateway's service,
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

    async fn gateway_service(&self, scope: RequestScope) -> Result<GatewayServiceStatus> {
        self.inner.gateway_service(scope).await
    }

    async fn change_gateway_service(
        &self,
        context: CommandContext,
        action: GatewayServiceAction,
    ) -> Result<GatewayServiceStatus> {
        let result = self
            .inner
            .change_gateway_service(context.clone(), action)
            .await;
        let operation = Operation::GatewayService {
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
    use crate::ContainerState;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use panel_errors::ErrorCode;
    use std::sync::Mutex;

    /// Refuses every change `intruder` asks for.
    struct Gateway;

    fn gateway(state: ContainerState) -> GatewayServiceStatus {
        GatewayServiceStatus {
            observed_at: None,
            container: GatewayContainer {
                engine: "docker".into(),
                container: ContainerSummary {
                    id: "g7".into(),
                    names: vec!["pingora-panel-gatewayd-1".into()],
                    image: "localhost/pingora-panel:dev".into(),
                    image_id: String::new(),
                    created: None,
                    state,
                    status: String::new(),
                    ports: Vec::new(),
                    labels: std::collections::BTreeMap::new(),
                    compose_project: Some("pingora-panel".into()),
                },
                started_at: None,
                finished_at: None,
                exit_code: 0,
                restarts: 0,
                health: String::new(),
            },
        }
    }

    #[async_trait]
    impl HostAgentPort for Gateway {
        async fn agent(&self, _: RequestScope) -> Result<AgentDescription> {
            Ok(AgentDescription::default())
        }

        async fn directories(&self, _: RequestScope) -> Result<DirectoriesReport> {
            Err(NoHostAgent::refusal())
        }

        async fn listeners(&self, _: RequestScope, _: Vec<u16>) -> Result<ListenersReport> {
            Err(NoHostAgent::refusal())
        }

        async fn gateway_service(&self, _: RequestScope) -> Result<GatewayServiceStatus> {
            Ok(gateway(ContainerState::Running))
        }

        async fn change_gateway_service(
            &self,
            context: CommandContext,
            action: GatewayServiceAction,
        ) -> Result<GatewayServiceStatus> {
            if context.actor() == "intruder" {
                return Err(PanelError::precondition_failed("the engine is disabled"));
            }
            Ok(gateway(if action == GatewayServiceAction::Stop {
                ContainerState::Exited
            } else {
                ContainerState::Running
            }))
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, std::result::Result<String, String>)>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::GatewayService { action, result } = operation {
                self.0.lock().unwrap().push((
                    action.as_str().to_owned(),
                    result
                        .map(|status| status.container.container.state.as_str().to_owned())
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
    async fn every_change_to_the_gateway_service_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let agent = RecordedHostAgent::new(Arc::new(Gateway), recorder.clone());
        agent
            .change_gateway_service(context("ops"), GatewayServiceAction::Stop)
            .await
            .unwrap();
        agent
            .change_gateway_service(context("intruder"), GatewayServiceAction::Restart)
            .await
            .unwrap_err();
        agent
            .gateway_service(RequestScope::new(RequestId::new("read").unwrap()))
            .await
            .unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                ("stop".to_owned(), Ok("exited".to_owned())),
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
