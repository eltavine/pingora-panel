use crate::{
    deployments::PgDeployments,
    reconcile::{Reconciliation, ReconciliationWatch},
};
use async_trait::async_trait;
use panel_application::{
    AbortOutcome, ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayStatus,
    GatewayUseCases, IdempotencyKey, IdempotencyLookup, PreparedDeployment, RequestScope,
};
use panel_errors::{PanelError, Result, ValidationReport};
use std::sync::Arc;

/// Records what reconciliation needs around publication: the document of
/// every prepared deployment, each activation's intent before it claims its
/// key, and the desired configuration once an activation succeeds. While
/// the gateway's configuration is quarantined, publication is refused.
pub struct RecordingUseCases {
    inner: Arc<dyn GatewayUseCases>,
    deployments: PgDeployments,
    reconciliation: ReconciliationWatch,
}

impl RecordingUseCases {
    pub fn new(
        inner: Arc<dyn GatewayUseCases>,
        deployments: PgDeployments,
        reconciliation: ReconciliationWatch,
    ) -> Self {
        Self {
            inner,
            deployments,
            reconciliation,
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
        self.admit()?;
        let prepared = self.inner.prepare(context, document.clone()).await?;
        self.deployments
            .record_prepared(&prepared, &document)
            .await?;
        Ok(prepared)
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.admit()?;
        self.deployments
            .record_intent(&context, &prepare_token, expected_active_hash.as_ref())
            .await?;
        let activated = self
            .inner
            .activate(context, prepare_token.clone(), expected_active_hash)
            .await?;
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
        self.inner.abort(context, prepare_token).await
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
