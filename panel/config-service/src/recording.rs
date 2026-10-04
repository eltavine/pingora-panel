use crate::{
    deployments::SqliteDeployments,
    reconcile::{Reconciliation, ReconciliationWatch},
};
use async_trait::async_trait;
use panel_application::{
    AbortOutcome, ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayStatus,
    GatewayUseCases, IdempotencyKey, IdempotencyLookup, PreparedDeployment, RequestScope,
};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_event_contracts::gateway::v1 as event;
use panel_events::EventData;
use panel_sqlite::EventLog;
use std::sync::Arc;

/// Records what reconciliation needs around publication: the document of
/// every prepared deployment, each activation's intent before it claims its
/// key, and the desired configuration once an activation succeeds. While
/// the gateway's configuration is quarantined, publication is refused.
/// Every preparation, activation and abort, and every refusal of one, is
/// published as an event.
pub struct RecordingUseCases {
    inner: Arc<dyn GatewayUseCases>,
    deployments: SqliteDeployments,
    reconciliation: ReconciliationWatch,
    events: EventLog,
}

/// The aggregate of snapshot events.
const SNAPSHOT: (&str, &str) = ("gateway", "snapshot");

impl RecordingUseCases {
    pub fn new(
        inner: Arc<dyn GatewayUseCases>,
        deployments: SqliteDeployments,
        reconciliation: ReconciliationWatch,
        events: EventLog,
    ) -> Self {
        Self {
            inner,
            deployments,
            reconciliation,
            events,
        }
    }

    /// Records what an operation on the gateway did, or why it was refused.
    async fn record<T, E: EventData>(
        &self,
        context: &CommandContext,
        operation: &str,
        result: &Result<T>,
        data: impl FnOnce(&T) -> E,
    ) {
        let (scope, actor) = (context.scope(), context.actor());
        match result {
            Ok(value) => {
                self.events
                    .record(SNAPSHOT, &scope, actor, &data(value))
                    .await;
            }
            Err(error) => {
                self.events
                    .record(
                        SNAPSHOT,
                        &scope,
                        actor,
                        &event::SnapshotRefused {
                            operation: operation.to_owned(),
                            code: error.code.as_str().to_owned(),
                            message: error.message.clone(),
                        },
                    )
                    .await;
            }
        }
    }

    fn admit(&self) -> Result<()> {
        match self.reconciliation.current() {
            Reconciliation::Quarantined(_) => Err(PanelError::unavailable(
                "publication is suspended until the gateway's configuration is reconciled",
            )),
            Reconciliation::Pending | Reconciliation::InSync => Ok(()),
        }
    }
}

#[async_trait]
impl GatewayUseCases for RecordingUseCases {
    async fn validate(&self, document: ConfigDocument) -> Result<ValidationReport> {
        self.inner.validate(document).await
    }

    async fn validate_with_scope(
        &self,
        scope: RequestScope,
        document: ConfigDocument,
    ) -> Result<ValidationReport> {
        self.inner.validate_with_scope(scope, document).await
    }

    async fn prepare(
        &self,
        context: CommandContext,
        document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        let result = async {
            self.admit()?;
            let prepared = self
                .inner
                .prepare(context.clone(), document.clone())
                .await?;
            self.deployments
                .record_prepared(&prepared, &document)
                .await?;
            Ok(prepared)
        }
        .await;
        self.record(
            &context,
            "prepared",
            &result,
            |prepared: &PreparedDeployment| event::SnapshotPrepared {
                revision_id: prepared.revision_id().get(),
                content_hash: prepared.content_hash().as_str().to_owned(),
            },
        )
        .await;
        result
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let result = async {
            self.admit()?;
            self.deployments
                .record_intent(&context, &prepare_token, expected_active_hash.as_ref())
                .await?;
            self.inner
                .activate(context.clone(), prepare_token.clone(), expected_active_hash)
                .await
        }
        .await;
        self.record(
            &context,
            "activated",
            &result,
            |activated: &ActivatedDeployment| event::SnapshotActivated {
                revision_id: activated.revision_id().get(),
                content_hash: activated.content_hash().as_str().to_owned(),
                previous_active_hash: activated
                    .previous_active_hash()
                    .map(|hash| hash.as_str().to_owned()),
            },
        )
        .await;
        let activated = result?;
        // The gateway already committed; a failure here only delays the
        // record until reconciliation adopts the gateway's configuration.
        if let Err(error) = self
            .deployments
            .record_activated(&prepare_token, &activated)
            .await
        {
            tracing::warn!(error_code = %error.code, "desired configuration not recorded");
        }
        Ok(activated)
    }

    async fn abort(&self, context: CommandContext, prepare_token: String) -> Result<AbortOutcome> {
        let result = self.inner.abort(context.clone(), prepare_token).await;
        self.record(&context, "aborted", &result, |_| event::SnapshotAborted {})
            .await;
        result
    }

    async fn status(&self) -> Result<GatewayStatus> {
        self.inner.status().await
    }

    async fn status_with_scope(&self, scope: RequestScope) -> Result<GatewayStatus> {
        self.inner.status_with_scope(scope).await
    }

    async fn activation_receipt(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        self.inner.activation_receipt(key).await
    }
}
