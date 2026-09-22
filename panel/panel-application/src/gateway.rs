use crate::{CommandContext, IdempotencyKey, IdempotencyLookup};
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
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub prepare_token: String,
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
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub previous_active_hash: Option<ContentHash>,
}

/// Transport-neutral gateway status projection used by REST and CLI adapters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GatewayStatus {
    ready: bool,
    message: Option<String>,
    active_revision_id: Option<RevisionId>,
    active_hash: Option<ContentHash>,
    prepared_count: usize,
    adapter_version: String,
    schema_version: String,
}

impl GatewayStatus {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ready: bool,
        message: Option<String>,
        active_revision_id: Option<RevisionId>,
        active_hash: Option<ContentHash>,
        prepared_count: usize,
        adapter_version: impl Into<String>,
        schema_version: impl Into<String>,
    ) -> Self {
        Self {
            ready,
            message,
            active_revision_id,
            active_hash,
            prepared_count,
            adapter_version: adapter_version.into(),
            schema_version: schema_version.into(),
        }
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn active_revision_id(&self) -> Option<RevisionId> {
        self.active_revision_id
    }

    pub fn active_hash(&self) -> Option<&ContentHash> {
        self.active_hash.as_ref()
    }

    pub fn prepared_count(&self) -> usize {
        self.prepared_count
    }

    pub fn adapter_version(&self) -> &str {
        &self.adapter_version
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }
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

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment>;

    /// Activates a prepared configuration. Invalid argument, validation,
    /// conflict, not found, precondition, unsupported capability, and identity
    /// errors must only describe rejection before commit. Adapters must map
    /// errors after a possible commit to an uncertain error instead. Timeouts,
    /// resource, storage, internal, and unknown errors do not prove non-commit.
    async fn activate(
        &self,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment>;

    async fn status(&self) -> Result<GatewayStatus> {
        Err(PanelError::unsupported_capability(
            "gateway status is not configured",
        ))
    }

    async fn prepare_with_context(
        &self,
        _context: CommandContext,
        snapshot: RuntimeSnapshot,
    ) -> Result<PreparedDeployment> {
        self.prepare(snapshot).await
    }

    async fn activate_with_context(
        &self,
        _context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.activate(prepare_token, expected_active_hash).await
    }
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

    /// Uses the precommit rejection and uncertain outcome contract of
    /// [`GatewayPort::activate`], including when implemented by a decorator.
    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment>;

    async fn status(&self) -> Result<GatewayStatus> {
        Err(PanelError::unsupported_capability(
            "gateway status is not configured",
        ))
    }

    async fn activation_receipt(&self, _key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        Err(PanelError::unsupported_capability(
            "activation receipt queries are not configured",
        ))
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
        self.gateway.prepare_with_context(context, snapshot).await
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.gateway
            .activate_with_context(context, prepare_token, expected_active_hash)
            .await
    }

    async fn status(&self) -> Result<GatewayStatus> {
        self.gateway.status().await
    }
}
