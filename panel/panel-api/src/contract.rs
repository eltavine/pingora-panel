use panel_application::{ActivatedDeployment, ConfigDocument, PreparedDeployment};
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
