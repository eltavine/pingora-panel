//! Activation replay and claim lifecycle, independent of transport and storage adapters.

use crate::{
    AbortOutcome, ActivatedDeployment, CommandContext, ConfigDocument, DeploymentOutcome,
    GatewayStatus, GatewayUseCases, IdempotencyClaim, IdempotencyKey, IdempotencyLookup,
    IdempotencyRecord, IdempotencyRepository, PreparedDeployment,
};
use async_trait::async_trait;
use panel_domain::ContentHash;
use panel_errors::{PanelError, Result, ValidationReport};
use std::sync::Arc;

mod failure;
#[cfg(test)]
mod tests;

/// Application decorator that makes activation retries replay a durable
/// receipt instead of executing the gateway mutation twice.
pub struct IdempotentGatewayUseCases {
    inner: Arc<dyn GatewayUseCases>,
    repository: Arc<dyn IdempotencyRepository>,
}

impl IdempotentGatewayUseCases {
    pub fn new(
        inner: Arc<dyn GatewayUseCases>,
        repository: Arc<dyn IdempotencyRepository>,
    ) -> Self {
        Self { inner, repository }
    }
}

fn activation_request_hash(
    prepare_token: &str,
    expected_active_hash: Option<&ContentHash>,
) -> ContentHash {
    let mut bytes = Vec::with_capacity(prepare_token.len() + 1 + 64);
    bytes.extend_from_slice(prepare_token.as_bytes());
    bytes.push(0);
    if let Some(hash) = expected_active_hash {
        bytes.extend_from_slice(hash.as_str().as_bytes());
    }
    ContentHash::from_bytes(&bytes)
}

fn replayed_activation(record: IdempotencyRecord) -> Result<ActivatedDeployment> {
    match record.outcome().clone() {
        DeploymentOutcome::Succeeded(deployment) => Ok(deployment),
        _ => Err(PanelError::internal(
            "idempotency record does not contain a replayable activation receipt",
        )),
    }
}

#[async_trait]
impl GatewayUseCases for IdempotentGatewayUseCases {
    async fn validate(&self, document: ConfigDocument) -> Result<ValidationReport> {
        self.inner.validate(document).await
    }

    async fn prepare(
        &self,
        context: CommandContext,
        document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        self.inner.prepare(context, document).await
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let key = context.idempotency_key().clone();
        let request_hash = activation_request_hash(&prepare_token, expected_active_hash.as_ref());
        match self.repository.claim(&key, &request_hash).await? {
            IdempotencyClaim::Replay(record) => return replayed_activation(record),
            IdempotencyClaim::InProgress => {
                return Err(PanelError::resource_exhausted(
                    "activation with this idempotency key is already in progress",
                ))
            }
            IdempotencyClaim::Conflict => {
                return Err(PanelError::conflict(
                    "idempotency key was already used for a different activation request",
                ))
            }
            IdempotencyClaim::Acquired => {}
        }

        let deployment = match self
            .inner
            .activate(context, prepare_token, expected_active_hash)
            .await
        {
            Ok(deployment) => deployment,
            Err(error) => {
                if failure::confirmed_before_commit(&error) {
                    // Failure to release leaves the claim protected. Never infer
                    // permission to resubmit from retryability alone.
                    self.repository.abort(&key, &request_hash).await?;
                }
                return Err(error);
            }
        };
        let record = IdempotencyRecord::new(
            request_hash.clone(),
            DeploymentOutcome::Succeeded(deployment.clone()),
        );
        if let Err(error) = self.repository.complete(&key, record).await {
            // The gateway has already acknowledged activation, but the durable
            // replay receipt could not be committed. Do not abort the claim:
            // doing so could permit a retry to execute the mutation twice.
            return Err(PanelError::commit_outcome_unknown(
                "activation succeeded but its idempotency receipt could not be persisted",
            )
            .with_source(error));
        }
        Ok(deployment)
    }

    async fn status(&self) -> Result<GatewayStatus> {
        self.inner.status().await
    }

    async fn activation_receipt(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        self.repository.lookup(key).await
    }

    async fn abort(&self, context: CommandContext, prepare_token: String) -> Result<AbortOutcome> {
        self.inner.abort(context, prepare_token).await
    }
}
