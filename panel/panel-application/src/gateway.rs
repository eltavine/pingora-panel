use crate::CommandContext;
use crate::{IdempotencyClaim, IdempotencyRecord, IdempotencyRepository};
use async_trait::async_trait;
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Format-neutral source passed from a transport adapter to a compiler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDocument {
    schema_version: String,
    media_type: String,
    body: Vec<u8>,
}

impl ConfigDocument {
    pub fn new(
        schema_version: impl Into<String>,
        media_type: impl Into<String>,
        body: Vec<u8>,
    ) -> Result<Self> {
        let schema_version = schema_version.into();
        let media_type = media_type.into();
        if schema_version.is_empty() || schema_version.len() > 64 || !schema_version.is_ascii() {
            return Err(PanelError::invalid_argument(
                "schema version must contain 1..=64 ASCII bytes",
            ));
        }
        if media_type.is_empty() || media_type.len() > 128 || !media_type.is_ascii() {
            return Err(PanelError::invalid_argument(
                "media type must contain 1..=128 ASCII bytes",
            ));
        }
        if body.is_empty() {
            return Err(PanelError::invalid_argument(
                "configuration document body must not be empty",
            ));
        }
        Ok(Self {
            schema_version,
            media_type,
            body,
        })
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Port for compiling a transport document into the versioned IR.
#[async_trait]
pub trait ConfigCompiler: Send + Sync {
    async fn compile(&self, document: ConfigDocument) -> Result<RuntimeSnapshot>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedDeployment {
    revision_id: RevisionId,
    content_hash: ContentHash,
    prepare_token: String,
}

impl PreparedDeployment {
    pub fn new(
        revision_id: RevisionId,
        content_hash: ContentHash,
        prepare_token: impl Into<String>,
    ) -> Result<Self> {
        let prepare_token = prepare_token.into();
        if prepare_token.is_empty() || prepare_token.len() > 512 {
            return Err(PanelError::invalid_argument(
                "prepare token must contain 1..=512 bytes",
            ));
        }
        Ok(Self {
            revision_id,
            content_hash,
            prepare_token,
        })
    }

    pub fn revision_id(&self) -> RevisionId {
        self.revision_id
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn prepare_token(&self) -> &str {
        &self.prepare_token
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivatedDeployment {
    revision_id: RevisionId,
    content_hash: ContentHash,
    previous_active_hash: Option<ContentHash>,
}

impl ActivatedDeployment {
    pub fn new(
        revision_id: RevisionId,
        content_hash: ContentHash,
        previous_active_hash: Option<ContentHash>,
    ) -> Self {
        Self {
            revision_id,
            content_hash,
            previous_active_hash,
        }
    }

    pub fn revision_id(&self) -> RevisionId {
        self.revision_id
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn previous_active_hash(&self) -> Option<&ContentHash> {
        self.previous_active_hash.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeploymentOutcome {
    Succeeded(ActivatedDeployment),
    Rejected(ValidationReport),
    FailedBeforeCommit,
    PendingReconciliation,
}

/// Replaceable gateway adapter port owned by the application layer.
#[async_trait]
pub trait GatewayPort: Send + Sync {
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport>;

    async fn prepare(
        &self,
        context: CommandContext,
        snapshot: RuntimeSnapshot,
    ) -> Result<PreparedDeployment>;

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment>;
}

/// Stable use-case facade consumed by HTTP, CLI and worker adapters.
#[async_trait]
pub trait GatewayUseCases: Send + Sync {
    async fn validate(&self, document: ConfigDocument) -> Result<ValidationReport>;

    async fn prepare(
        &self,
        context: CommandContext,
        document: ConfigDocument,
    ) -> Result<PreparedDeployment>;

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment>;
}

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
                let _ = self.repository.abort(&key, &request_hash).await;
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
}

/// Default application workflow over independently replaceable ports.
pub struct GatewayService {
    gateway: Arc<dyn GatewayPort>,
    compiler: Arc<dyn ConfigCompiler>,
}

impl GatewayService {
    pub fn new(gateway: Arc<dyn GatewayPort>, compiler: Arc<dyn ConfigCompiler>) -> Self {
        Self { gateway, compiler }
    }
}

#[async_trait]
impl GatewayUseCases for GatewayService {
    async fn validate(&self, document: ConfigDocument) -> Result<ValidationReport> {
        let snapshot = self.compiler.compile(document).await?;
        self.gateway.validate(snapshot).await
    }

    async fn prepare(
        &self,
        context: CommandContext,
        document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        let snapshot = self.compiler.compile(document).await?;
        self.gateway.prepare(context, snapshot).await
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.gateway
            .activate(context, prepare_token, expected_active_hash)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdempotencyKey, RequestDeadline, RequestId};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct FakeUseCases {
        activations: AtomicUsize,
    }

    #[async_trait]
    impl GatewayUseCases for FakeUseCases {
        async fn validate(&self, _document: ConfigDocument) -> Result<ValidationReport> {
            Ok(ValidationReport::valid())
        }

        async fn prepare(
            &self,
            _context: CommandContext,
            _document: ConfigDocument,
        ) -> Result<PreparedDeployment> {
            Err(PanelError::internal("not used in idempotency test"))
        }

        async fn activate(
            &self,
            _context: CommandContext,
            _prepare_token: String,
            _expected_active_hash: Option<ContentHash>,
        ) -> Result<ActivatedDeployment> {
            self.activations.fetch_add(1, Ordering::SeqCst);
            Ok(ActivatedDeployment::new(
                RevisionId::new(1),
                ContentHash::from_bytes(b"active"),
                None,
            ))
        }
    }

    struct MemoryIdempotency {
        value: Mutex<Option<(IdempotencyKey, ContentHash, Option<IdempotencyRecord>)>>,
        fail_complete: bool,
    }

    #[async_trait]
    impl IdempotencyRepository for MemoryIdempotency {
        async fn claim(
            &self,
            key: &IdempotencyKey,
            request_hash: &ContentHash,
        ) -> Result<IdempotencyClaim> {
            let mut value = self.value.lock().unwrap();
            match value.as_ref() {
                None => {
                    *value = Some((key.clone(), request_hash.clone(), None));
                    Ok(IdempotencyClaim::Acquired)
                }
                Some((stored_key, stored_hash, record))
                    if stored_key == key && stored_hash == request_hash =>
                {
                    Ok(record
                        .clone()
                        .map_or(IdempotencyClaim::InProgress, IdempotencyClaim::Replay))
                }
                Some((stored_key, _, _)) if stored_key == key => Ok(IdempotencyClaim::Conflict),
                Some(_) => Ok(IdempotencyClaim::Conflict),
            }
        }

        async fn complete(&self, key: &IdempotencyKey, record: IdempotencyRecord) -> Result<()> {
            if self.fail_complete {
                return Err(PanelError::storage_unavailable(
                    "test receipt store unavailable",
                ));
            }
            let mut value = self.value.lock().unwrap();
            if let Some((stored_key, _, stored_record)) = value.as_mut() {
                if stored_key == key {
                    *stored_record = Some(record);
                    return Ok(());
                }
            }
            Err(PanelError::internal("missing idempotency claim"))
        }

        async fn abort(&self, key: &IdempotencyKey, _request_hash: &ContentHash) -> Result<()> {
            let mut value = self.value.lock().unwrap();
            if value
                .as_ref()
                .is_some_and(|(stored_key, _, record)| stored_key == key && record.is_none())
            {
                *value = None;
            }
            Ok(())
        }
    }

    fn context(key: &str) -> CommandContext {
        CommandContext::new(
            RequestId::new("req-1").unwrap(),
            RequestId::new("corr-1").unwrap(),
            "tester",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new(key).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn activation_replays_receipt_and_rejects_hash_reuse() {
        let gateway = Arc::new(FakeUseCases {
            activations: AtomicUsize::new(0),
        });
        let repository = Arc::new(MemoryIdempotency {
            value: Mutex::new(None),
            fail_complete: false,
        });
        let service = IdempotentGatewayUseCases::new(gateway.clone(), repository);

        let first = service
            .activate(context("idem-1"), "prepare-1".into(), None)
            .await
            .unwrap();
        let second = service
            .activate(context("idem-1"), "prepare-1".into(), None)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(gateway.activations.load(Ordering::SeqCst), 1);

        let conflict = service
            .activate(context("idem-1"), "prepare-2".into(), None)
            .await
            .unwrap_err();
        assert_eq!(conflict.code.as_str(), panel_errors::ErrorCode::CONFLICT);
    }

    #[tokio::test]
    async fn atomic_claim_blocks_a_concurrent_duplicate_before_gateway_execution() {
        let repository = MemoryIdempotency {
            value: Mutex::new(None),
            fail_complete: false,
        };
        let key = IdempotencyKey::new("idem-claim").unwrap();
        let hash = ContentHash::from_bytes(b"request");
        assert_eq!(
            repository.claim(&key, &hash).await.unwrap(),
            IdempotencyClaim::Acquired
        );
        assert_eq!(
            repository.claim(&key, &hash).await.unwrap(),
            IdempotencyClaim::InProgress
        );

        let deployment =
            ActivatedDeployment::new(RevisionId::new(1), ContentHash::from_bytes(b"active"), None);
        repository
            .complete(
                &key,
                IdempotencyRecord::new(hash.clone(), DeploymentOutcome::Succeeded(deployment)),
            )
            .await
            .unwrap();
        assert!(matches!(
            repository.claim(&key, &hash).await.unwrap(),
            IdempotencyClaim::Replay(_)
        ));
    }

    #[tokio::test]
    async fn receipt_failure_never_releases_claim_after_gateway_success() {
        let gateway = Arc::new(FakeUseCases {
            activations: AtomicUsize::new(0),
        });
        let repository = Arc::new(MemoryIdempotency {
            value: Mutex::new(None),
            fail_complete: true,
        });
        let service = IdempotentGatewayUseCases::new(gateway.clone(), repository);

        let error = service
            .activate(context("idem-receipt-failure"), "prepare-1".into(), None)
            .await
            .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::COMMIT_OUTCOME_UNKNOWN
        );
        assert!(error.retryable);
        assert_eq!(gateway.activations.load(Ordering::SeqCst), 1);
    }
}
