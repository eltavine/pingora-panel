use crate::{deployments::SqliteDeployments, SqliteActivationReceipts};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use panel_application::{
    CommandContext, DeploymentOutcome, GatewayUseCases, IdempotencyKey, IdempotencyRecord,
    IdempotencyRepository, RequestDeadline, RequestId,
};
use panel_errors::{ErrorCode, PanelError, Result};
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ACTOR: &str = "config-service";
const DEADLINE: Duration = Duration::from_secs(60);
const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// How the gateway's active configuration relates to the desired one.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reconciliation {
    /// Not compared yet, or the last attempt could not reach a dependency.
    Pending,
    /// The gateway runs the desired configuration, or nothing is desired.
    InSync,
    /// The gateway runs a configuration this service did not activate;
    /// publication stays suspended until an operator resolves it.
    Quarantined(String),
}

/// The published reconciliation state.
#[derive(Clone)]
pub struct ReconciliationWatch(watch::Receiver<Reconciliation>);

impl ReconciliationWatch {
    pub fn current(&self) -> Reconciliation {
        self.0.borrow().clone()
    }
}

/// Reconciliation as a degrading readiness check.
pub struct ReconciliationCheck(pub ReconciliationWatch);

#[async_trait]
impl HealthCheck for ReconciliationCheck {
    fn component(&self) -> &str {
        "reconciliation"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Component
    }

    async fn check(&self) -> CheckOutcome {
        match self.0.current() {
            Reconciliation::InSync => CheckOutcome::pass(),
            Reconciliation::Pending => CheckOutcome::warn("not reconciled yet"),
            Reconciliation::Quarantined(_) => {
                CheckOutcome::fail("the gateway runs an unknown configuration")
            }
        }
    }
}

/// Brings the gateway and this service's records into agreement after a
/// crash or a gateway that lost its state:
///
/// 1. activations that claimed their key without recording a receipt are
///    re-issued with the same token and expected hash; the gateway replays
///    the receipt of one that committed, and one that did not either runs
///    now or is released for the caller to retry;
/// 2. a gateway with no active configuration receives the desired one;
/// 3. a gateway running a newer configuration prepared here is adopted;
/// 4. any other configuration is quarantined.
pub struct Reconciler {
    gateway: Arc<dyn GatewayUseCases>,
    receipts: Arc<SqliteActivationReceipts>,
    deployments: SqliteDeployments,
    state: watch::Sender<Reconciliation>,
}

impl Reconciler {
    /// `gateway` must not record receipts itself.
    pub fn new(
        gateway: Arc<dyn GatewayUseCases>,
        receipts: Arc<SqliteActivationReceipts>,
        deployments: SqliteDeployments,
    ) -> (Self, ReconciliationWatch) {
        let (state, receiver) = watch::channel(Reconciliation::Pending);
        (
            Self {
                gateway,
                receipts,
                deployments,
                state,
            },
            ReconciliationWatch(receiver),
        )
    }

    /// Reconciles once, publishing and returning the outcome. Errors leave
    /// the previous state and are retried by [`Self::run`].
    pub async fn reconcile(&self) -> Result<Reconciliation> {
        for pending in self.deployments.pending().await? {
            let context = context(
                pending.idempotency_key.clone(),
                pending.correlation_id.clone(),
                &pending.actor,
            )?;
            match self
                .gateway
                .activate(
                    context,
                    pending.prepare_token.clone(),
                    pending.expected_active_hash.clone(),
                )
                .await
            {
                Ok(activated) => {
                    tracing::info!(
                        idempotency_key = %pending.idempotency_key,
                        revision_id = activated.revision_id().get(),
                        "completed an interrupted activation"
                    );
                    self.receipts
                        .complete(
                            &pending.idempotency_key,
                            IdempotencyRecord::new(
                                pending.request_hash.clone(),
                                DeploymentOutcome::Succeeded(activated.clone()),
                            ),
                        )
                        .await?;
                    self.deployments
                        .record_activated(&pending.prepare_token, &activated)
                        .await?;
                }
                Err(error) if did_not_commit(&error) => {
                    tracing::info!(
                        idempotency_key = %pending.idempotency_key,
                        error_code = %error.code,
                        "released an interrupted activation that did not commit"
                    );
                    self.receipts
                        .abort(&pending.idempotency_key, &pending.request_hash)
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }

        let active = self.gateway.status().await?.active_hash().cloned();
        let outcome = match (self.deployments.desired().await?, active) {
            (None, _) => Reconciliation::InSync,
            (Some(desired), Some(active)) if desired.content_hash == active => {
                Reconciliation::InSync
            }
            (Some(desired), None) => {
                let scope = format!(
                    "{}-{}",
                    desired.content_hash.as_str(),
                    desired.revision_id.get()
                );
                let prepared = self
                    .gateway
                    .prepare(
                        context(
                            IdempotencyKey::new(format!("reconcile-prepare-{scope}"))?,
                            RequestId::new(format!("reconcile-{}", Uuid::now_v7()))?,
                            ACTOR,
                        )?,
                        desired.document.clone(),
                    )
                    .await?;
                let activated = self
                    .gateway
                    .activate(
                        context(
                            IdempotencyKey::new(format!("reconcile-activate-{scope}"))?,
                            RequestId::new(format!("reconcile-{}", Uuid::now_v7()))?,
                            ACTOR,
                        )?,
                        prepared.prepare_token().into(),
                        None,
                    )
                    .await?;
                self.deployments
                    .record_prepared(&prepared, &desired.document)
                    .await?;
                self.deployments
                    .record_activated(prepared.prepare_token(), &activated)
                    .await?;
                tracing::warn!(
                    revision_id = activated.revision_id().get(),
                    content_hash = %activated.content_hash(),
                    "restored the desired configuration to a gateway without one"
                );
                Reconciliation::InSync
            }
            (Some(desired), Some(active)) => {
                match self.deployments.prepared_with_hash(&active).await? {
                    Some(newer) if newer.revision_id > desired.revision_id => {
                        self.deployments
                            .record_activated(
                                &newer.prepare_token,
                                &panel_application::ActivatedDeployment::new(
                                    newer.revision_id,
                                    newer.content_hash.clone(),
                                    Some(desired.content_hash.clone()),
                                ),
                            )
                            .await?;
                        tracing::info!(
                            revision_id = newer.revision_id.get(),
                            "adopted a newer configuration the gateway confirmed"
                        );
                        Reconciliation::InSync
                    }
                    _ => {
                        tracing::error!(
                            gateway_hash = %active,
                            desired_hash = %desired.content_hash,
                            "the gateway runs a configuration this service did not activate; \
                             publication is suspended"
                        );
                        Reconciliation::Quarantined(format!(
                            "gateway runs {active}, desired {}",
                            desired.content_hash
                        ))
                    }
                }
            }
        };
        self.state.send_replace(outcome.clone());
        Ok(outcome)
    }

    /// Reconciles at start and then every `interval`, retrying failures
    /// sooner, and purges records nothing needs anymore.
    pub async fn run(self, interval: Duration, cancel: CancellationToken) {
        let mut failing = false;
        loop {
            let delay = match self.reconcile().await {
                Ok(_) => {
                    failing = false;
                    if let Err(error) = self.deployments.purge(RETENTION).await {
                        tracing::warn!(error_code = %error.code, "deployment record purge failed");
                    }
                    interval
                }
                Err(error) => {
                    if !failing {
                        tracing::warn!(
                            error_code = %error.code,
                            error = %error.message,
                            "reconciliation failed; retrying"
                        );
                    }
                    failing = true;
                    interval.min(Duration::from_secs(1))
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
        }
    }
}

/// Failures with which the gateway proves the activation did not commit.
fn did_not_commit(error: &PanelError) -> bool {
    matches!(
        error.code.as_str(),
        ErrorCode::NOT_FOUND | ErrorCode::CONFLICT | ErrorCode::VALIDATION_FAILED
    ) || error.is_confirmed_precommit()
}

fn context(key: IdempotencyKey, correlation_id: RequestId, actor: &str) -> Result<CommandContext> {
    let deadline = (Utc::now() + DEADLINE).to_rfc3339_opts(SecondsFormat::Secs, true);
    CommandContext::new(
        RequestId::new(format!("reconcile-{}", Uuid::now_v7()))?,
        correlation_id,
        actor,
        RequestDeadline::new(deadline)?,
        key,
    )
}
