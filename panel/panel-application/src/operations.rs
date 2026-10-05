//! Records of operations whose effect lives outside the recording service,
//! such as calls to the gateway, for the audit trail.

use crate::{
    BackupChange, CommandContext, ComposeAction, ComposeChange, ContainerAction, ContainerChange,
    ContainerEngine, DataPlaneState, FileChecks, GatewayRuntimePort, GatewayServiceAction,
    GatewayServiceStatus, ImagePulled, ImageRemoval, LogDeletion, PruneReport, RequestScope,
    SiteFileChange, SitePath, UpstreamHealth, UpstreamHealthReport,
};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::sync::Arc;

/// An operation whose effect lives outside the recording service, with its
/// outcome: what it changed, or why it was refused.
#[derive(Clone, Copy, Debug)]
pub enum Operation<'a> {
    Reload(std::result::Result<&'a DataPlaneState, &'a PanelError>),
    SetWorkerCount(std::result::Result<&'a DataPlaneState, &'a PanelError>),
    Shutdown(std::result::Result<(), &'a PanelError>),
    SetEndpointDrained {
        upstream: &'a str,
        endpoint: &'a str,
        drained: bool,
        result: std::result::Result<(), &'a PanelError>,
    },
    /// The log source was asked to delete records of `site`, or of every site.
    DeleteLogs {
        site: Option<&'a str>,
        result: std::result::Result<&'a LogDeletion, &'a PanelError>,
    },
    /// The host agent was asked to start, stop or restart the service that
    /// runs the gateway.
    GatewayService {
        action: GatewayServiceAction,
        result: std::result::Result<&'a GatewayServiceStatus, &'a PanelError>,
    },
    /// The host agent was asked to enable or disable a container engine.
    ContainerEngine {
        engine: &'a str,
        enabled: bool,
        result: std::result::Result<&'a ContainerEngine, &'a PanelError>,
    },
    /// The host agent was asked to act on a container.
    Container {
        engine: &'a str,
        /// The container as the request named it.
        container: &'a str,
        action: ContainerAction,
        result: std::result::Result<&'a ContainerChange, &'a PanelError>,
    },
    /// The host agent was asked to remove a reference to an image.
    ImageRemoval {
        engine: &'a str,
        /// The image as the request named it.
        image: &'a str,
        force: bool,
        result: std::result::Result<&'a ImageRemoval, &'a PanelError>,
    },
    /// A file or directory of the static sites was asked to change.
    SiteFile {
        path: &'a SitePath,
        change: SiteFileChange<'a>,
    },
    /// A backup was asked for, removed or restored; `id` is empty when a
    /// request was refused before a backup was listed.
    Backup {
        id: &'a str,
        change: BackupChange<'a>,
    },
    /// The host agent was asked to pull an image.
    ImagePull {
        engine: &'a str,
        /// The reference as the request named it.
        reference: &'a str,
        result: std::result::Result<&'a ImagePulled, &'a PanelError>,
    },
    /// The host agent was asked to prune an engine.
    EnginePrune {
        engine: &'a str,
        result: std::result::Result<&'a PruneReport, &'a PanelError>,
    },
    /// The host agent was asked to bring a Compose project up, down or
    /// restart it.
    ComposeProject {
        engine: &'a str,
        /// The project as the request named it.
        project: &'a str,
        action: ComposeAction,
        result: std::result::Result<&'a ComposeChange, &'a PanelError>,
    },
}

/// Where operations are recorded.
#[async_trait]
pub trait OperationLog: Send + Sync {
    /// Records `operation`. What it describes already happened, so
    /// recording does not fail the caller.
    async fn record(&self, context: &CommandContext, operation: Operation<'_>);
}

/// A runtime port that records each operation that changes the data plane,
/// refused or not.
pub struct RecordedRuntime {
    inner: Arc<dyn GatewayRuntimePort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedRuntime {
    pub fn new(inner: Arc<dyn GatewayRuntimePort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl GatewayRuntimePort for RecordedRuntime {
    async fn data_plane(&self, scope: RequestScope) -> Result<DataPlaneState> {
        self.inner.data_plane(scope).await
    }

    async fn reload(&self, context: CommandContext) -> Result<DataPlaneState> {
        let result = self.inner.reload(context.clone()).await;
        self.log
            .record(&context, Operation::Reload(result.as_ref()))
            .await;
        result
    }

    async fn set_worker_count(
        &self,
        context: CommandContext,
        workers: u32,
    ) -> Result<DataPlaneState> {
        let result = self.inner.set_worker_count(context.clone(), workers).await;
        self.log
            .record(&context, Operation::SetWorkerCount(result.as_ref()))
            .await;
        result
    }

    async fn shutdown(&self, context: CommandContext) -> Result<()> {
        let result = self.inner.shutdown(context.clone()).await;
        self.log
            .record(&context, Operation::Shutdown(result.as_ref().map(|_| ())))
            .await;
        result
    }

    async fn upstream_health(&self, scope: RequestScope) -> Result<UpstreamHealthReport> {
        self.inner.upstream_health(scope).await
    }

    async fn file_checks(&self, scope: RequestScope) -> Result<FileChecks> {
        self.inner.file_checks(scope).await
    }

    async fn set_endpoint_drained(
        &self,
        context: CommandContext,
        upstream: String,
        endpoint: String,
        drained: bool,
    ) -> Result<UpstreamHealth> {
        let result = self
            .inner
            .set_endpoint_drained(context.clone(), upstream.clone(), endpoint.clone(), drained)
            .await;
        let operation = Operation::SetEndpointDrained {
            upstream: &upstream,
            endpoint: &endpoint,
            drained,
            result: result.as_ref().map(|_| ()),
        };
        self.log.record(&context, operation).await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdempotencyKey, RequestDeadline, RequestId};
    use panel_errors::PanelError;
    use std::sync::Mutex;

    struct Runtime;

    #[async_trait]
    impl GatewayRuntimePort for Runtime {
        async fn data_plane(&self, _scope: RequestScope) -> Result<DataPlaneState> {
            Ok(DataPlaneState::default())
        }

        async fn reload(&self, _context: CommandContext) -> Result<DataPlaneState> {
            Ok(DataPlaneState {
                generation: 3,
                worker_count: 2,
                ..DataPlaneState::default()
            })
        }

        async fn set_worker_count(
            &self,
            _context: CommandContext,
            _workers: u32,
        ) -> Result<DataPlaneState> {
            Err(PanelError::invalid_argument(
                "workers must be between 1 and 64",
            ))
        }

        async fn shutdown(&self, _context: CommandContext) -> Result<()> {
            Ok(())
        }

        async fn upstream_health(&self, _scope: RequestScope) -> Result<UpstreamHealthReport> {
            Ok(UpstreamHealthReport::default())
        }

        async fn set_endpoint_drained(
            &self,
            _context: CommandContext,
            upstream: String,
            _endpoint: String,
            _drained: bool,
        ) -> Result<UpstreamHealth> {
            Ok(UpstreamHealth {
                upstream_id: upstream,
                ..UpstreamHealth::default()
            })
        }
    }

    /// An operation as the test log keeps it: what, and its error code if
    /// it was refused.
    type Recorded = (String, String, std::result::Result<String, String>);

    #[derive(Default)]
    struct Log(Mutex<Vec<Recorded>>);

    #[async_trait]
    impl OperationLog for Log {
        async fn record(&self, context: &CommandContext, operation: Operation<'_>) {
            let code = |error: &PanelError| error.code.as_str().to_owned();
            let (name, outcome) = match operation {
                Operation::Reload(result) => (
                    "reload".to_owned(),
                    result.map(|state| format!("{}/{}", state.generation, state.worker_count)),
                ),
                Operation::SetWorkerCount(result) => (
                    "workers".to_owned(),
                    result.map(|state| state.worker_count.to_string()),
                ),
                Operation::Shutdown(result) => {
                    ("shutdown".to_owned(), result.map(|()| String::new()))
                }
                Operation::SetEndpointDrained {
                    upstream,
                    endpoint,
                    drained,
                    result,
                } => (
                    format!("drained={drained}"),
                    result.map(|()| format!("{upstream}/{endpoint}")),
                ),
                Operation::DeleteLogs { site, result } => (
                    "delete-logs".to_owned(),
                    result.map(|_| site.unwrap_or("*").to_owned()),
                ),
                Operation::GatewayService { action, result } => (
                    format!("gateway-service-{}", action.as_str()),
                    result.map(|status| status.container.container.state.as_str().to_owned()),
                ),
                Operation::ContainerEngine {
                    engine,
                    enabled,
                    result,
                } => (
                    format!("engine-{engine}"),
                    result.map(|_| enabled.to_string()),
                ),
                Operation::Container {
                    container,
                    action,
                    result,
                    ..
                } => (
                    format!("container-{}", action.as_str()),
                    result.map(|_| container.to_owned()),
                ),
                Operation::ImageRemoval { image, result, .. } => {
                    ("image-remove".to_owned(), result.map(|_| image.to_owned()))
                }
                Operation::SiteFile { path, change } => match change {
                    SiteFileChange::Written(result) => (
                        "file-write".to_owned(),
                        result.map(|written| written.size_bytes.to_string()),
                    ),
                    SiteFileChange::DirectoryCreated(result) => (
                        "directory-create".to_owned(),
                        result.map(|()| path.to_string()),
                    ),
                    SiteFileChange::Removed { result, .. } => (
                        "file-remove".to_owned(),
                        result.map(|removal| removal.removed.to_string()),
                    ),
                },
                Operation::Backup { id, change } => match change {
                    BackupChange::Requested(result) => (
                        "backup-request".to_owned(),
                        result.map(|backup| backup.id.clone()),
                    ),
                    BackupChange::Deleted(result) => {
                        ("backup-delete".to_owned(), result.map(|()| id.to_owned()))
                    }
                    BackupChange::SitesRestored { result, .. } => (
                        "backup-restore-sites".to_owned(),
                        result.map(|restored| restored.files.to_string()),
                    ),
                    BackupChange::ConfigurationRestored(result) => (
                        "backup-restore-configuration".to_owned(),
                        result.map(|version| version.to_string()),
                    ),
                },
                Operation::ImagePull {
                    reference, result, ..
                } => (
                    "image-pull".to_owned(),
                    result.map(|_| reference.to_owned()),
                ),
                Operation::EnginePrune { engine, result } => (
                    format!("prune-{engine}"),
                    result.map(|report| report.reclaimed_bytes.to_string()),
                ),
                Operation::ComposeProject {
                    project,
                    action,
                    result,
                    ..
                } => (
                    format!("compose-{}", action.as_str()),
                    result.map(|_| project.to_owned()),
                ),
            };
            self.0
                .lock()
                .unwrap()
                .push((context.actor().to_owned(), name, outcome.map_err(code)));
        }
    }

    fn context() -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn changes_and_refusals_are_recorded_and_reads_are_not() {
        let log = Arc::new(Log::default());
        let runtime = RecordedRuntime::new(Arc::new(Runtime), log.clone());
        runtime.data_plane(context().scope()).await.unwrap();
        runtime.reload(context()).await.unwrap();
        runtime.set_worker_count(context(), 0).await.unwrap_err();
        runtime.shutdown(context()).await.unwrap();
        runtime
            .set_endpoint_drained(context(), "pool".into(), "node".into(), true)
            .await
            .unwrap();
        let recorded = log.0.lock().unwrap().clone();
        let ops = |name: &str, outcome: std::result::Result<&str, &str>| {
            (
                "ops".to_owned(),
                name.to_owned(),
                outcome.map(str::to_owned).map_err(str::to_owned),
            )
        };
        assert_eq!(
            recorded,
            [
                ops("reload", Ok("3/2")),
                ops("workers", Err("INVALID_ARGUMENT")),
                ops("shutdown", Ok("")),
                ops("drained=true", Ok("pool/node")),
            ]
        );
    }
}
