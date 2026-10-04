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
}

/// A containers port that records each change to an engine, refused or not.
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
    }

    /// A change as the test log keeps it: the engine, whether it was being
    /// enabled, and the code it was refused with.
    type Recorded = (String, bool, std::result::Result<(), String>);

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Recorded>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::ContainerEngine {
                engine,
                enabled,
                result,
            } = operation
            {
                self.0.lock().unwrap().push((
                    engine.to_owned(),
                    enabled,
                    result
                        .map(|_| ())
                        .map_err(|error| error.code.as_str().to_owned()),
                ));
            }
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
                ("docker".to_owned(), false, Ok(())),
                (
                    "containerd".to_owned(),
                    true,
                    Err(ErrorCode::NOT_FOUND.to_owned())
                ),
            ]
        );
    }
}
