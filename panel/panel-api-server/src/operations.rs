//! Gateway operations and refused requests recorded in this service's
//! outbox for the audit trail.

use async_trait::async_trait;
use panel_api::{AccessAudit, Refusal};
use panel_application::{
    BackupChange, CommandContext, ComposeAction, ContainerAction, DataPlaneState,
    GatewayServiceAction, Operation, OperationLog, RequestScope, SiteFileChange,
};
use panel_errors::PanelError;
use panel_event_contracts::{
    backups::v1 as backups, containers::v1 as containers, files::v1 as files,
    gateway::v1 as gateway, host::v1 as host, identity::v1 as identity,
};
use panel_events::EventData;
use panel_identity::Principal;
use panel_sqlite::EventLog;

/// The data plane as a whole, the target of its operations.
const DATA_PLANE: (&str, &str) = ("gateway", "data-plane");
/// The service on the host that runs the gateway.
const GATEWAY_SERVICE: (&str, &str) = ("host", "gateway-service");
/// The aggregate types of container events, by engine and by container.
const ENGINE: &str = "container_engine";
const CONTAINER: &str = "container";
const IMAGE: &str = "container_image";
const PROJECT: &str = "compose_project";
/// The aggregate type of events about the static sites' files, by path.
const SITE_FILE: &str = "site_file";
const BACKUP: &str = "backup";

pub struct OutboxOperations(pub EventLog);

impl OutboxOperations {
    /// Records `outcome`, or the refusal of the operation that would have
    /// published it.
    async fn outcome<E: EventData>(
        &self,
        context: &CommandContext,
        target: (&str, &str),
        outcome: Result<E, &PanelError>,
    ) {
        let (scope, actor) = (context.scope(), context.actor());
        match outcome {
            Ok(data) => self.0.record(target, &scope, actor, &data).await,
            Err(error) => {
                let refused = gateway::OperationRefused {
                    operation: E::TYPE.trim_start_matches("gateway.").to_owned(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                };
                self.0.record(target, &scope, actor, &refused).await;
            }
        }
    }
}

fn reloaded(state: &DataPlaneState) -> gateway::Reloaded {
    gateway::Reloaded {
        generation: state.generation,
        workers: state.worker_count,
    }
}

fn workers_changed(state: &DataPlaneState) -> gateway::WorkersChanged {
    gateway::WorkersChanged {
        generation: state.generation,
        workers: state.worker_count,
    }
}

#[async_trait]
impl OperationLog for OutboxOperations {
    async fn record(&self, context: &CommandContext, operation: Operation<'_>) {
        match operation {
            Operation::Reload(result) => {
                self.outcome(context, DATA_PLANE, result.map(reloaded))
                    .await;
            }
            Operation::SetWorkerCount(result) => {
                self.outcome(context, DATA_PLANE, result.map(workers_changed))
                    .await;
            }
            Operation::Shutdown(result) => {
                let outcome = result.map(|()| gateway::ShutdownRequested {});
                self.outcome(context, DATA_PLANE, outcome).await;
            }
            Operation::SetEndpointDrained {
                upstream,
                endpoint,
                drained,
                result,
            } => {
                let target = ("upstream", upstream);
                let (upstream, endpoint) = (upstream.to_owned(), endpoint.to_owned());
                if drained {
                    let outcome = result.map(|()| gateway::EndpointDrained { upstream, endpoint });
                    self.outcome(context, target, outcome).await;
                } else {
                    let outcome = result.map(|()| gateway::EndpointRestored { upstream, endpoint });
                    self.outcome(context, target, outcome).await;
                }
            }
            Operation::DeleteLogs { site, result } => {
                let target = site.map_or(("gateway", "logs"), |site| ("site", site));
                let outcome = result.map(|deletion| gateway::LogsDeleted {
                    site: site.unwrap_or_default().to_owned(),
                    since: Some(chrono::DateTime::<chrono::Utc>::from(deletion.since).into()),
                    until: Some(chrono::DateTime::<chrono::Utc>::from(deletion.until).into()),
                });
                self.outcome(context, target, outcome).await;
            }
            Operation::GatewayService { action, result } => {
                let (scope, actor) = (context.scope(), context.actor());
                match result {
                    Ok(status) => {
                        let gateway = &status.container.container;
                        let container = gateway.names.first().cloned().unwrap_or_default();
                        let state = gateway.state.as_str().to_owned();
                        match action {
                            GatewayServiceAction::Start => {
                                let data = host::GatewayServiceStarted { container, state };
                                self.0.record(GATEWAY_SERVICE, &scope, actor, &data).await;
                            }
                            GatewayServiceAction::Stop => {
                                let data = host::GatewayServiceStopped { container, state };
                                self.0.record(GATEWAY_SERVICE, &scope, actor, &data).await;
                            }
                            GatewayServiceAction::Restart => {
                                let data = host::GatewayServiceRestarted { container, state };
                                self.0.record(GATEWAY_SERVICE, &scope, actor, &data).await;
                            }
                        }
                    }
                    Err(error) => {
                        let refused = host::OperationRefused {
                            operation: format!("gateway_service.{}", action.as_str()),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                        };
                        self.0
                            .record(GATEWAY_SERVICE, &scope, actor, &refused)
                            .await;
                    }
                }
            }
            Operation::ContainerEngine {
                engine,
                enabled,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target = (ENGINE, engine);
                let engine = engine.to_owned();
                match (result, enabled) {
                    (Ok(_), true) => {
                        let data = containers::EngineEnabled { engine };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    (Ok(_), false) => {
                        let data = containers::EngineDisabled { engine };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    (Err(error), _) => {
                        let refused = containers::OperationRefused {
                            engine,
                            operation: if enabled {
                                "engine.enable"
                            } else {
                                "engine.disable"
                            }
                            .to_owned(),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            ..containers::OperationRefused::default()
                        };
                        self.0.record(target, &scope, actor, &refused).await;
                    }
                }
            }
            Operation::Container {
                engine,
                container,
                action,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                match result {
                    Ok(change) => {
                        let target_id = format!("{engine}/{}", change.name);
                        let target = (CONTAINER, target_id.as_str());
                        let (engine, id, name) =
                            (engine.to_owned(), change.id.clone(), change.name.clone());
                        match action {
                            ContainerAction::Start => {
                                let data = containers::ContainerStarted { engine, id, name };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ContainerAction::Stop => {
                                let data = containers::ContainerStopped { engine, id, name };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ContainerAction::Restart => {
                                let data = containers::ContainerRestarted { engine, id, name };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ContainerAction::Kill => {
                                let data = containers::ContainerKilled { engine, id, name };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ContainerAction::Remove { force, volumes } => {
                                let data = containers::ContainerRemoved {
                                    engine,
                                    id,
                                    name,
                                    force,
                                    remove_volumes: volumes,
                                };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                        }
                    }
                    Err(error) => {
                        let target_id = format!("{engine}/{container}");
                        let refused = containers::OperationRefused {
                            engine: engine.to_owned(),
                            operation: format!("container.{}", action.as_str()),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            container: container.to_owned(),
                            ..containers::OperationRefused::default()
                        };
                        self.0
                            .record((CONTAINER, &target_id), &scope, actor, &refused)
                            .await;
                    }
                }
            }
            Operation::ComposeProject {
                engine,
                project,
                action,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target_id = format!("{engine}/{project}");
                let target = (PROJECT, target_id.as_str());
                match result {
                    Ok(change) => {
                        let (engine, project) = (engine.to_owned(), project.to_owned());
                        let failed: Vec<String> = change
                            .failures
                            .iter()
                            .map(|failure| failure.name.clone())
                            .collect();
                        let changed = change.changed;
                        match action {
                            ComposeAction::Up => {
                                let data = containers::ComposeUp {
                                    engine,
                                    project,
                                    changed,
                                    failed,
                                };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ComposeAction::Down => {
                                let data = containers::ComposeDown {
                                    engine,
                                    project,
                                    changed,
                                    failed,
                                };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                            ComposeAction::Restart => {
                                let data = containers::ComposeRestarted {
                                    engine,
                                    project,
                                    changed,
                                    failed,
                                };
                                self.0.record(target, &scope, actor, &data).await;
                            }
                        }
                    }
                    Err(error) => {
                        let refused = containers::OperationRefused {
                            engine: engine.to_owned(),
                            operation: format!("compose.{}", action.as_str()),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            project: project.to_owned(),
                            ..containers::OperationRefused::default()
                        };
                        self.0.record(target, &scope, actor, &refused).await;
                    }
                }
            }
            Operation::EnginePrune { engine, result } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target = (ENGINE, engine);
                match result {
                    Ok(report) => {
                        let data = containers::EnginePruned {
                            engine: engine.to_owned(),
                            removed: report
                                .outcomes
                                .iter()
                                .filter(|outcome| outcome.refusal.is_none())
                                .map(|outcome| {
                                    format!("{} {}", outcome.item.kind.as_str(), outcome.item.name)
                                })
                                .collect(),
                            kept: u32::try_from(
                                report
                                    .outcomes
                                    .iter()
                                    .filter(|outcome| outcome.refusal.is_some())
                                    .count(),
                            )
                            .unwrap_or(u32::MAX),
                            reclaimed_bytes: report.reclaimed_bytes,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    Err(error) => {
                        let refused = containers::OperationRefused {
                            engine: engine.to_owned(),
                            operation: "engine.prune".to_owned(),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            ..containers::OperationRefused::default()
                        };
                        self.0.record(target, &scope, actor, &refused).await;
                    }
                }
            }
            Operation::ImageRemoval {
                engine,
                image,
                force,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target_id = format!("{engine}/{image}");
                let target = (IMAGE, target_id.as_str());
                match result {
                    Ok(removal) => {
                        let data = containers::ImageRemoved {
                            engine: engine.to_owned(),
                            id: removal.id.clone(),
                            image: image.to_owned(),
                            untagged: removal.untagged.clone(),
                            deleted: removal.deleted.clone(),
                            force,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    Err(error) => {
                        let refused = containers::OperationRefused {
                            engine: engine.to_owned(),
                            operation: "image.remove".to_owned(),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            image: image.to_owned(),
                            ..containers::OperationRefused::default()
                        };
                        self.0.record(target, &scope, actor, &refused).await;
                    }
                }
            }
            Operation::SiteFile { path, change } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target_id = path.to_string();
                let target = (SITE_FILE, target_id.as_str());
                let refused = |operation: &str, error: &PanelError| files::OperationRefused {
                    operation: operation.to_owned(),
                    path: target_id.clone(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                };
                match change {
                    SiteFileChange::Written(Ok(written)) => {
                        let data = files::FileWritten {
                            path: target_id.clone(),
                            size_bytes: written.size_bytes,
                            sha256: written.sha256.clone(),
                            created: written.created,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    SiteFileChange::Written(Err(error)) => {
                        let data = refused("file.write", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    SiteFileChange::DirectoryCreated(Ok(())) => {
                        let data = files::DirectoryCreated {
                            path: target_id.clone(),
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    SiteFileChange::DirectoryCreated(Err(error)) => {
                        let data = refused("directory.create", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    SiteFileChange::Removed {
                        recursive,
                        result: Ok(removal),
                    } => {
                        let data = files::EntryRemoved {
                            path: target_id.clone(),
                            kind: removal.kind.as_str().to_owned(),
                            recursive,
                            removed: removal.removed,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    SiteFileChange::Removed {
                        result: Err(error), ..
                    } => {
                        let data = refused("entry.remove", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                }
            }
            Operation::Backup { id, change } => {
                let (scope, actor) = (context.scope(), context.actor());
                let refused = |operation: &str, error: &PanelError| backups::OperationRefused {
                    operation: operation.to_owned(),
                    backup_id: id.to_owned(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                };
                let target = (BACKUP, if id.is_empty() { "-" } else { id });
                match change {
                    BackupChange::Requested(Ok(backup)) => {
                        let data = backups::BackupRequested {
                            backup_id: backup.id.clone(),
                            contents: backup
                                .contents
                                .iter()
                                .map(|content| content.as_str().to_owned())
                                .collect(),
                            site_path: backup.site_path.clone(),
                        };
                        self.0
                            .record((BACKUP, backup.id.as_str()), &scope, actor, &data)
                            .await;
                    }
                    BackupChange::Requested(Err(error)) => {
                        let data = refused("archive.request", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::Deleted(Ok(())) => {
                        let data = backups::BackupDeleted {
                            backup_id: id.to_owned(),
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::Deleted(Err(error)) => {
                        let data = refused("archive.delete", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::SitesRestored {
                        site_path,
                        result: Ok(restored),
                    } => {
                        let data = backups::SitesRestored {
                            backup_id: id.to_owned(),
                            site_path: site_path.to_owned(),
                            files: restored.files,
                            bytes: restored.bytes,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::SitesRestored {
                        result: Err(error), ..
                    } => {
                        let data = refused("sites.restore", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::ConfigurationRestored(Ok(draft_version)) => {
                        let data = backups::ConfigurationRestored {
                            backup_id: id.to_owned(),
                            draft_version,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    BackupChange::ConfigurationRestored(Err(error)) => {
                        let data = refused("configuration.restore", error);
                        self.0.record(target, &scope, actor, &data).await;
                    }
                }
            }
            Operation::ImagePull {
                engine,
                reference,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target_id = format!("{engine}/{reference}");
                let target = (IMAGE, target_id.as_str());
                match result {
                    Ok(pulled) => {
                        let data = containers::ImagePulled {
                            engine: engine.to_owned(),
                            reference: reference.to_owned(),
                            id: pulled.image.id.clone(),
                            digest: pulled.digest.clone().unwrap_or_default(),
                            updated: pulled.updated,
                        };
                        self.0.record(target, &scope, actor, &data).await;
                    }
                    Err(error) => {
                        let refused = containers::OperationRefused {
                            engine: engine.to_owned(),
                            operation: "image.pull".to_owned(),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                            image: reference.to_owned(),
                            ..containers::OperationRefused::default()
                        };
                        self.0.record(target, &scope, actor, &refused).await;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl AccessAudit for OutboxOperations {
    async fn denied(&self, principal: &Principal, refusal: &Refusal, scope: &RequestScope) {
        let data = identity::AccessDenied {
            method: refusal.method.clone(),
            route: refusal.route.clone(),
            reason: refusal.reason.to_owned(),
            permission: refusal.permission.map(str::to_owned),
        };
        self.0
            .record(
                ("account", &principal.account.to_string()),
                scope,
                principal.actor(),
                &data,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_events::AggregateType;

    #[test]
    fn every_aggregate_type_is_valid() {
        for kind in [
            DATA_PLANE.0,
            GATEWAY_SERVICE.0,
            ENGINE,
            CONTAINER,
            IMAGE,
            PROJECT,
            SITE_FILE,
            BACKUP,
            "upstream",
            "gateway",
            "site",
        ] {
            assert!(AggregateType::new(kind).is_ok(), "{kind}");
        }
    }
}
