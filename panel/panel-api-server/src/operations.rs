//! Gateway operations and refused requests recorded in this service's
//! outbox for the audit trail.

use async_trait::async_trait;
use panel_api::{AccessAudit, Refusal};
use panel_application::{CommandContext, DataPlaneState, Operation, OperationLog, RequestScope};
use panel_errors::PanelError;
use panel_event_contracts::{gateway::v1 as gateway, identity::v1 as identity};
use panel_events::EventData;
use panel_identity::Principal;
use panel_postgres::EventLog;

/// The data plane as a whole, the target of its operations.
const DATA_PLANE: (&str, &str) = ("gateway", "data-plane");

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
