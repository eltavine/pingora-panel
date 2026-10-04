//! Gateway operations and refused requests recorded in this service's
//! outbox for the audit trail.

use async_trait::async_trait;
use panel_api::{AccessAudit, Refusal};
use panel_application::{
    CommandContext, ContainerAction, DataPlaneState, Operation, OperationLog, RequestScope,
    UnitAction,
};
use panel_errors::PanelError;
use panel_event_contracts::{
    containers::v1 as containers, gateway::v1 as gateway, host::v1 as host,
    identity::v1 as identity,
};
use panel_events::EventData;
use panel_identity::Principal;
use panel_sqlite::EventLog;

/// The data plane as a whole, the target of its operations.
const DATA_PLANE: (&str, &str) = ("gateway", "data-plane");
/// The gateway's systemd unit on the host.
const GATEWAY_UNIT: (&str, &str) = ("host", "gateway-unit");

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
            Operation::GatewayUnit { action, result } => {
                let (scope, actor) = (context.scope(), context.actor());
                match result {
                    Ok(status) => {
                        let (unit, active_state) =
                            (status.name.clone(), status.active_state.clone());
                        match action {
                            UnitAction::Start => {
                                let data = host::GatewayUnitStarted { unit, active_state };
                                self.0.record(GATEWAY_UNIT, &scope, actor, &data).await;
                            }
                            UnitAction::Stop => {
                                let data = host::GatewayUnitStopped { unit, active_state };
                                self.0.record(GATEWAY_UNIT, &scope, actor, &data).await;
                            }
                            UnitAction::Restart => {
                                let data = host::GatewayUnitRestarted { unit, active_state };
                                self.0.record(GATEWAY_UNIT, &scope, actor, &data).await;
                            }
                        }
                    }
                    Err(error) => {
                        let refused = host::OperationRefused {
                            operation: format!("gateway_unit.{}", action.as_str()),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                        };
                        self.0.record(GATEWAY_UNIT, &scope, actor, &refused).await;
                    }
                }
            }
            Operation::ContainerEngine {
                engine,
                enabled,
                result,
            } => {
                let (scope, actor) = (context.scope(), context.actor());
                let target = ("container-engine", engine);
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
                            container: String::new(),
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
                        let target = ("container", target_id.as_str());
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
                        };
                        self.0
                            .record(("container", &target_id), &scope, actor, &refused)
                            .await;
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
