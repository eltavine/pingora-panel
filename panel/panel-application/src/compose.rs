//! The Compose projects on the container engines `ops-agent` reaches
//! (ADR 0031), known by the labels Compose puts on their containers.

use crate::{
    CommandContext, ContainerLogLine, ContainerLogQuery, Operation, OperationLog, RequestScope,
};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{sync::Arc, time::SystemTime};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectService {
    pub name: String,
    pub containers: u32,
    pub running: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposeProject {
    pub name: String,
    /// Where Compose ran, on the host.
    pub working_directory: Option<String>,
    /// The files Compose read, on the host.
    pub config_files: Vec<String>,
    /// By name.
    pub services: Vec<ProjectService>,
    pub containers: u32,
    pub running: u32,
    /// The panel's own installation, which is only ever brought up.
    pub installation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposeProjectList {
    pub observed_at: Option<SystemTime>,
    /// By name.
    pub projects: Vec<ComposeProject>,
}

/// What the panel does to a Compose project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposeAction {
    /// Starts the containers that are not running.
    Up,
    /// Stops and removes the containers and the project's networks, and
    /// keeps its volumes.
    Down,
    Restart,
}

impl ComposeAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Restart => "restart",
        }
    }
}

/// What the engine refused, when it refused only some of an action.
#[derive(Clone, Debug)]
pub struct ComposeFailure {
    /// A container's or network's name.
    pub name: String,
    pub error: PanelError,
}

#[derive(Clone, Debug)]
pub struct ComposeChange {
    /// The project afterwards; `None` once it is down.
    pub project: Option<ComposeProject>,
    /// How many containers the action changed.
    pub changed: u32,
    pub failures: Vec<ComposeFailure>,
}

/// A line one of a project's containers printed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposeLogLine {
    pub service: String,
    pub container: String,
    pub line: ContainerLogLine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposeLogs {
    pub observed_at: Option<SystemTime>,
    /// Oldest first.
    pub lines: Vec<ComposeLogLine>,
    /// Older lines were left out to keep within the agent's size limit.
    pub truncated: bool,
}

/// A Compose file a project's labels name.
#[derive(Clone, Debug)]
pub struct ComposeFile {
    /// On the host.
    pub path: String,
    /// What it holds, or why it could not be read.
    pub content: std::result::Result<String, PanelError>,
}

#[async_trait]
pub trait ComposePort: Send + Sync {
    async fn projects(&self, scope: RequestScope, engine: String) -> Result<ComposeProjectList>;

    /// Brings a project up, down or restarts it.
    async fn act_on_project(
        &self,
        context: CommandContext,
        engine: String,
        project: String,
        action: ComposeAction,
    ) -> Result<ComposeChange>;

    /// The last lines a project's containers printed, merged by time.
    async fn project_logs(
        &self,
        scope: RequestScope,
        engine: String,
        project: String,
        query: ContainerLogQuery,
    ) -> Result<ComposeLogs>;

    /// The Compose files a project's labels name.
    async fn project_files(
        &self,
        scope: RequestScope,
        engine: String,
        project: String,
    ) -> Result<Vec<ComposeFile>>;
}

/// The port of an installation whose agent manages no engine.
pub struct NoCompose;

impl NoCompose {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("no host agent manages container engines here")
    }
}

#[async_trait]
impl ComposePort for NoCompose {
    async fn projects(&self, _: RequestScope, _: String) -> Result<ComposeProjectList> {
        Err(Self::refusal())
    }

    async fn act_on_project(
        &self,
        _: CommandContext,
        _: String,
        _: String,
        _: ComposeAction,
    ) -> Result<ComposeChange> {
        Err(Self::refusal())
    }

    async fn project_logs(
        &self,
        _: RequestScope,
        _: String,
        _: String,
        _: ContainerLogQuery,
    ) -> Result<ComposeLogs> {
        Err(Self::refusal())
    }

    async fn project_files(
        &self,
        _: RequestScope,
        _: String,
        _: String,
    ) -> Result<Vec<ComposeFile>> {
        Err(Self::refusal())
    }
}

/// A Compose port that records each action, refused or not.
pub struct RecordedCompose {
    inner: Arc<dyn ComposePort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedCompose {
    pub fn new(inner: Arc<dyn ComposePort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl ComposePort for RecordedCompose {
    async fn projects(&self, scope: RequestScope, engine: String) -> Result<ComposeProjectList> {
        self.inner.projects(scope, engine).await
    }

    async fn act_on_project(
        &self,
        context: CommandContext,
        engine: String,
        project: String,
        action: ComposeAction,
    ) -> Result<ComposeChange> {
        let result = self
            .inner
            .act_on_project(context.clone(), engine.clone(), project.clone(), action)
            .await;
        let operation = Operation::ComposeProject {
            engine: &engine,
            project: &project,
            action,
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn project_logs(
        &self,
        scope: RequestScope,
        engine: String,
        project: String,
        query: ContainerLogQuery,
    ) -> Result<ComposeLogs> {
        self.inner.project_logs(scope, engine, project, query).await
    }

    async fn project_files(
        &self,
        scope: RequestScope,
        engine: String,
        project: String,
    ) -> Result<Vec<ComposeFile>> {
        self.inner.project_files(scope, engine, project).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use std::sync::Mutex;

    /// Restarts `shop` and refuses the rest.
    struct Projects;

    #[async_trait]
    impl ComposePort for Projects {
        async fn projects(&self, _: RequestScope, _: String) -> Result<ComposeProjectList> {
            Err(NoCompose::refusal())
        }

        async fn act_on_project(
            &self,
            _: CommandContext,
            _: String,
            project: String,
            _: ComposeAction,
        ) -> Result<ComposeChange> {
            if project != "shop" {
                return Err(PanelError::not_found(format!(
                    "no Compose project named {project}"
                )));
            }
            Ok(ComposeChange {
                project: None,
                changed: 2,
                failures: Vec::new(),
            })
        }

        async fn project_logs(
            &self,
            _: RequestScope,
            _: String,
            _: String,
            _: ContainerLogQuery,
        ) -> Result<ComposeLogs> {
            Err(NoCompose::refusal())
        }

        async fn project_files(
            &self,
            _: RequestScope,
            _: String,
            _: String,
        ) -> Result<Vec<ComposeFile>> {
            Err(NoCompose::refusal())
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::ComposeProject {
                engine,
                project,
                action,
                result,
            } = operation
            {
                let outcome = match result {
                    Ok(change) => change.changed.to_string(),
                    Err(error) => error.code.as_str().to_owned(),
                };
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("{engine}/{project} {} {outcome}", action.as_str()));
            }
        }
    }

    #[tokio::test]
    async fn every_action_on_a_project_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let compose = RecordedCompose::new(Arc::new(Projects), recorder.clone());
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap();
        compose
            .act_on_project(
                context.clone(),
                "docker".into(),
                "shop".into(),
                ComposeAction::Restart,
            )
            .await
            .unwrap();
        compose
            .act_on_project(
                context,
                "docker".into(),
                "ghost".into(),
                ComposeAction::Down,
            )
            .await
            .unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            ["docker/shop restart 2", "docker/ghost down NOT_FOUND"]
        );
    }
}
