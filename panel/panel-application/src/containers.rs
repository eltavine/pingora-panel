//! The container engines `ops-agent` reaches and what runs on them
//! (ADR 0031).

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineVersion {
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub architecture: String,
    pub kernel_version: String,
    pub go_version: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineInfo {
    pub containers: u32,
    pub running: u32,
    pub paused: u32,
    pub stopped: u32,
    pub images: u32,
    pub storage_driver: String,
    pub cgroup_driver: String,
    pub operating_system: String,
    pub cpus: u32,
    pub memory_bytes: u64,
    pub name: String,
}

/// A Docker or Podman engine the agent is configured with.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContainerEngine {
    /// `docker` or `podman`.
    pub id: String,
    pub socket: String,
    pub enabled: bool,
    pub reachable: bool,
    /// Why it is unreachable.
    pub detail: String,
    pub version: Option<EngineVersion>,
    pub info: Option<EngineInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Exited,
    Removing,
    Dead,
    Stopping,
    /// A state this build does not know.
    Unknown,
}

impl ContainerState {
    /// The engine's word for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Exited => "exited",
            Self::Removing => "removing",
            Self::Dead => "dead",
            Self::Stopping => "stopping",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortMapping {
    pub private_port: u16,
    /// The host's port, when published.
    pub public_port: Option<u16>,
    pub host_ip: String,
    /// `tcp`, `udp` or `sctp`.
    pub protocol: String,
}

/// A container as a list shows it, without its command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerSummary {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub image_id: String,
    pub created: Option<SystemTime>,
    pub state: ContainerState,
    pub status: String,
    pub ports: Vec<PortMapping>,
    pub labels: BTreeMap<String, String>,
    pub compose_project: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContainerFilter {
    /// Matched against names and images, ignoring case.
    pub search: String,
    /// Every state when empty.
    pub states: Vec<ContainerState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerList {
    pub observed_at: Option<SystemTime>,
    pub containers: Vec<ContainerSummary>,
}

/// What the panel does to a container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerAction {
    Start,
    /// The container's stop signal, then SIGKILL once its stop timeout
    /// passes.
    Stop,
    Restart,
    /// SIGKILL at once.
    Kill,
    Remove {
        /// A running container is killed and removed rather than refused.
        force: bool,
        /// Its anonymous volumes go with it.
        volumes: bool,
    },
}

impl ContainerAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Kill => "kill",
            Self::Remove { .. } => "remove",
        }
    }
}

/// Where a container's storage comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerMount {
    /// `volume`, `bind`, `tmpfs` and so on.
    pub kind: String,
    /// The volume's name, for a volume.
    pub name: Option<String>,
    pub source: String,
    pub destination: String,
    pub read_write: bool,
}

/// A network a container is attached to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerNetwork {
    pub name: String,
    pub ip_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub gateway: Option<String>,
    pub mac_address: Option<String>,
    pub aliases: Vec<String>,
}

/// What inspecting a container shows, without its environment or command
/// line, which carry secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerDetail {
    pub container: ContainerSummary,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    /// How it last stopped; 0 until it has.
    pub exit_code: i64,
    /// Why it last failed, in the engine's words.
    pub error: Option<String>,
    pub oom_killed: bool,
    /// How often the engine restarted it under its restart policy.
    pub restarts: u32,
    /// `healthy`, `unhealthy` or `starting`.
    pub health: Option<String>,
    /// `no`, `always`, `unless-stopped` or `on-failure`.
    pub restart_policy: Option<String>,
    /// How often `on-failure` restarts it; 0 for no limit.
    pub restart_retries: u32,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub working_directory: Option<String>,
    pub platform: Option<String>,
    pub mounts: Vec<ContainerMount>,
    pub networks: Vec<ContainerNetwork>,
}

/// A container an action was taken on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerChange {
    pub id: String,
    /// Its name before the action.
    pub name: String,
    /// The container afterwards; `None` once removed.
    pub container: Option<ContainerSummary>,
}

#[async_trait]
pub trait ContainersPort: Send + Sync {
    async fn engines(&self, scope: RequestScope) -> Result<Vec<ContainerEngine>>;

    /// Enables or disables an engine; the agent keeps the choice.
    async fn set_engine(
        &self,
        context: CommandContext,
        engine: String,
        enabled: bool,
    ) -> Result<ContainerEngine>;

    async fn containers(
        &self,
        scope: RequestScope,
        engine: String,
        filter: ContainerFilter,
    ) -> Result<ContainerList>;

    /// A container's configuration and state, named by its ID or name.
    async fn inspect(
        &self,
        scope: RequestScope,
        engine: String,
        container: String,
    ) -> Result<ContainerDetail>;

    /// Starts, stops, restarts, kills or removes a container, named by its
    /// ID or name.
    async fn act(
        &self,
        context: CommandContext,
        engine: String,
        container: String,
        action: ContainerAction,
    ) -> Result<ContainerChange>;
}

/// The port of an installation whose agent manages no engine.
pub struct NoContainers;

impl NoContainers {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("no host agent manages container engines here")
    }
}

#[async_trait]
impl ContainersPort for NoContainers {
    async fn engines(&self, _: RequestScope) -> Result<Vec<ContainerEngine>> {
        Err(Self::refusal())
    }

    async fn set_engine(&self, _: CommandContext, _: String, _: bool) -> Result<ContainerEngine> {
        Err(Self::refusal())
    }

    async fn containers(
        &self,
        _: RequestScope,
        _: String,
        _: ContainerFilter,
    ) -> Result<ContainerList> {
        Err(Self::refusal())
    }

    async fn inspect(&self, _: RequestScope, _: String, _: String) -> Result<ContainerDetail> {
        Err(Self::refusal())
    }

    async fn act(
        &self,
        _: CommandContext,
        _: String,
        _: String,
        _: ContainerAction,
    ) -> Result<ContainerChange> {
        Err(Self::refusal())
    }
}

/// A containers port that records each change to an engine or a
/// container, refused or not.
pub struct RecordedContainers {
    inner: Arc<dyn ContainersPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedContainers {
    pub fn new(inner: Arc<dyn ContainersPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl ContainersPort for RecordedContainers {
    async fn engines(&self, scope: RequestScope) -> Result<Vec<ContainerEngine>> {
        self.inner.engines(scope).await
    }

    async fn set_engine(
        &self,
        context: CommandContext,
        engine: String,
        enabled: bool,
    ) -> Result<ContainerEngine> {
        let result = self
            .inner
            .set_engine(context.clone(), engine.clone(), enabled)
            .await;
        let operation = Operation::ContainerEngine {
            engine: &engine,
            enabled,
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn containers(
        &self,
        scope: RequestScope,
        engine: String,
        filter: ContainerFilter,
    ) -> Result<ContainerList> {
        self.inner.containers(scope, engine, filter).await
    }

    async fn inspect(
        &self,
        scope: RequestScope,
        engine: String,
        container: String,
    ) -> Result<ContainerDetail> {
        self.inner.inspect(scope, engine, container).await
    }

    async fn act(
        &self,
        context: CommandContext,
        engine: String,
        container: String,
        action: ContainerAction,
    ) -> Result<ContainerChange> {
        let result = self
            .inner
            .act(context.clone(), engine.clone(), container.clone(), action)
            .await;
        let operation = Operation::Container {
            engine: &engine,
            container: &container,
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

    /// Knows only the `docker` engine.
    struct Engines;

    #[async_trait]
    impl ContainersPort for Engines {
        async fn engines(&self, _: RequestScope) -> Result<Vec<ContainerEngine>> {
            Ok(Vec::new())
        }

        async fn set_engine(
            &self,
            _: CommandContext,
            engine: String,
            enabled: bool,
        ) -> Result<ContainerEngine> {
            if engine != "docker" {
                return Err(PanelError::not_found(format!("no engine named {engine}")));
            }
            Ok(ContainerEngine {
                id: engine,
                enabled,
                ..ContainerEngine::default()
            })
        }

        async fn containers(
            &self,
            _: RequestScope,
            _: String,
            _: ContainerFilter,
        ) -> Result<ContainerList> {
            Err(NoContainers::refusal())
        }

        async fn inspect(&self, _: RequestScope, _: String, _: String) -> Result<ContainerDetail> {
            Err(NoContainers::refusal())
        }

        async fn act(
            &self,
            _: CommandContext,
            _: String,
            container: String,
            _: ContainerAction,
        ) -> Result<ContainerChange> {
            if container != "shop-web-1" {
                return Err(PanelError::not_found(format!("no container {container}")));
            }
            Ok(ContainerChange {
                id: "b2".into(),
                name: container,
                container: None,
            })
        }
    }

    /// A change as the test log keeps it: what changed, and the code it was
    /// refused with.
    type Recorded = (String, std::result::Result<(), String>);

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Recorded>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            let code = |error: &PanelError| error.code.as_str().to_owned();
            let recorded = match operation {
                Operation::ContainerEngine {
                    engine,
                    enabled,
                    result,
                } => (
                    format!("{engine} enabled={enabled}"),
                    result.map(|_| ()).map_err(code),
                ),
                Operation::Container {
                    engine,
                    container,
                    action,
                    result,
                } => (
                    format!("{engine}/{container} {}", action.as_str()),
                    result.map(|_| ()).map_err(code),
                ),
                _ => return,
            };
            self.0.lock().unwrap().push(recorded);
        }
    }

    #[tokio::test]
    async fn every_change_to_an_engine_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let containers = RecordedContainers::new(Arc::new(Engines), recorder.clone());
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap();
        containers
            .set_engine(context.clone(), "docker".into(), false)
            .await
            .unwrap();
        containers
            .set_engine(context, "containerd".into(), true)
            .await
            .unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                ("docker enabled=false".to_owned(), Ok(())),
                (
                    "containerd enabled=true".to_owned(),
                    Err(ErrorCode::NOT_FOUND.to_owned())
                ),
            ]
        );
    }

    #[tokio::test]
    async fn every_action_on_a_container_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let containers = RecordedContainers::new(Arc::new(Engines), recorder.clone());
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap();
        let removed = containers
            .act(
                context.clone(),
                "docker".into(),
                "shop-web-1".into(),
                ContainerAction::Remove {
                    force: true,
                    volumes: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(removed.id, "b2");
        containers
            .act(
                context,
                "docker".into(),
                "ghost".into(),
                ContainerAction::Restart,
            )
            .await
            .unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                ("docker/shop-web-1 remove".to_owned(), Ok(())),
                (
                    "docker/ghost restart".to_owned(),
                    Err(ErrorCode::NOT_FOUND.to_owned())
                ),
            ]
        );
    }
}
