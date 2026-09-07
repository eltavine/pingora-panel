use panel_application::{
    ActivatedDeployment, ConfigDocument, DeploymentOutcome, GatewayStatus, IdempotencyRecord,
    PreparedDeployment,
};
use panel_errors::{Diagnostic, PanelError, ValidationReport};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SnapshotEnvelope {
    /// Selects the compiler/schema reader before the document is interpreted.
    pub schema_version: String,
    /// Versioned source document interpreted by the injected ConfigCompiler.
    pub snapshot: serde_json::Value,
}

impl TryFrom<SnapshotEnvelope> for ConfigDocument {
    type Error = PanelError;

    fn try_from(value: SnapshotEnvelope) -> Result<Self, Self::Error> {
        let body = serde_json::to_vec(&value.snapshot).map_err(|error| {
            PanelError::invalid_argument(format!("configuration cannot be encoded: {error}"))
        })?;
        ConfigDocument::new(value.schema_version, "application/json", body)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ActivateRequest {
    pub prepare_token: String,
    #[serde(default)]
    pub expected_active_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct DiagnosticDetails {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_span: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl From<Diagnostic> for DiagnosticDetails {
    fn from(value: Diagnostic) -> Self {
        Self {
            code: value.code.to_string(),
            message: value.message,
            source_span: value.source_span,
            resource_id: value.resource_id,
            help: value.help,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ValidationResponse {
    pub valid: bool,
    pub diagnostics: Vec<DiagnosticDetails>,
}

impl From<ValidationReport> for ValidationResponse {
    fn from(value: ValidationReport) -> Self {
        Self {
            valid: value.valid,
            diagnostics: value.diagnostics.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct PreparedResponse {
    pub revision_id: u64,
    pub content_hash: String,
    pub prepare_token: String,
}

impl From<PreparedDeployment> for PreparedResponse {
    fn from(value: PreparedDeployment) -> Self {
        Self {
            revision_id: value.revision_id().get(),
            content_hash: value.content_hash().to_string(),
            prepare_token: value.prepare_token().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ActivatedResponse {
    pub revision_id: u64,
    pub content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_active_hash: Option<String>,
}

impl From<ActivatedDeployment> for ActivatedResponse {
    fn from(value: ActivatedDeployment) -> Self {
        Self {
            revision_id: value.revision_id().get(),
            content_hash: value.content_hash().to_string(),
            previous_active_hash: value.previous_active_hash().map(ToString::to_string),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct GatewayStatusResponse {
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_revision_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_hash: Option<String>,
    pub prepared_count: usize,
    pub adapter_version: String,
    pub schema_version: String,
}

impl From<GatewayStatus> for GatewayStatusResponse {
    fn from(value: GatewayStatus) -> Self {
        Self {
            ready: value.ready(),
            message: value.message().map(str::to_owned),
            active_revision_id: value.active_revision_id().map(|id| id.get()),
            active_hash: value.active_hash().map(ToString::to_string),
            prepared_count: value.prepared_count(),
            adapter_version: value.adapter_version().to_owned(),
            schema_version: value.schema_version().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct IdempotencyReceiptResponse {
    pub request_hash: String,
    pub outcome: ReceiptOutcomeResponse,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct IdempotencyReceiptPendingResponse {
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReceiptOutcomeResponse {
    Succeeded {
        revision_id: u64,
        content_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_active_hash: Option<String>,
    },
    Rejected {
        diagnostics: Vec<DiagnosticDetails>,
    },
    FailedBeforeCommit,
    PendingReconciliation,
    Unknown,
}

impl From<IdempotencyRecord> for IdempotencyReceiptResponse {
    fn from(value: IdempotencyRecord) -> Self {
        Self {
            request_hash: value.request_hash().to_string(),
            outcome: ReceiptOutcomeResponse::from(value.outcome()),
        }
    }
}

impl From<&DeploymentOutcome> for ReceiptOutcomeResponse {
    fn from(value: &DeploymentOutcome) -> Self {
        match value {
            DeploymentOutcome::Succeeded(deployment) => Self::Succeeded {
                revision_id: deployment.revision_id().get(),
                content_hash: deployment.content_hash().to_string(),
                previous_active_hash: deployment.previous_active_hash().map(ToString::to_string),
            },
            DeploymentOutcome::Rejected(report) => Self::Rejected {
                diagnostics: report.diagnostics.iter().cloned().map(Into::into).collect(),
            },
            DeploymentOutcome::FailedBeforeCommit => Self::FailedBeforeCommit,
            DeploymentOutcome::PendingReconciliation => Self::PendingReconciliation,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_errors: Vec<DiagnosticDetails>,
}
