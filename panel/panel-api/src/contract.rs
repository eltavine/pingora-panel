use panel_application::{
    AbortOutcome, ActivatedDeployment, ConfigDocument, DeploymentOutcome, GatewayStatus,
    IdempotencyRecord, PreparedDeployment,
};
use panel_errors::{Diagnostic, DiagnosticSeverity, PanelError, ValidationReport};
use panel_platform::ServiceListing;
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
    /// Current active content hash required by compare-and-swap. Omit or use
    /// null only for the first activation, when no configuration is active.
    #[serde(default)]
    pub expected_active_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AbortRequest {
    pub prepare_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct AbortResponse {
    pub aborted: bool,
}

impl From<AbortOutcome> for AbortResponse {
    fn from(outcome: AbortOutcome) -> Self {
        Self {
            aborted: outcome.aborted(),
        }
    }
}

/// How serious a diagnostic is; only errors block a change.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl From<DiagnosticSeverity> for Severity {
    fn from(value: DiagnosticSeverity) -> Self {
        match value {
            DiagnosticSeverity::Info => Self::Info,
            DiagnosticSeverity::Warning => Self::Warning,
            DiagnosticSeverity::Error => Self::Error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct DiagnosticDetails {
    pub code: String,
    pub severity: Severity,
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
            severity: value.severity.into(),
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

/// Live service instances at one moment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ServiceListingResponse {
    /// RFC 3339 time at which the directory was read.
    pub observed_at: String,
    pub services: Vec<ServiceInstanceResponse>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ServiceInstanceResponse {
    pub service: String,
    pub instance_id: String,
    pub build_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// RFC 3339 start time of the instance.
    pub started_at: String,
    pub protocols: Vec<ProtocolSupportResponse>,
    pub capabilities: Vec<CapabilityResponse>,
}

/// The revisions of one protocol package an instance speaks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProtocolSupportResponse {
    pub name: String,
    pub min_revision: u32,
    pub max_revision: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct CapabilityResponse {
    pub name: String,
    pub version: String,
}

impl From<ServiceListing> for ServiceListingResponse {
    fn from(value: ServiceListing) -> Self {
        let time = |time: chrono::DateTime<chrono::Utc>| {
            time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        };
        Self {
            observed_at: time(value.observed_at()),
            services: value
                .services()
                .iter()
                .map(|instance| ServiceInstanceResponse {
                    service: instance.service().to_string(),
                    instance_id: instance.instance_id().to_string(),
                    build_version: instance.build_version().into(),
                    schema_version: (!instance.schema_version().is_empty())
                        .then(|| instance.schema_version().into()),
                    started_at: time(instance.started_at()),
                    protocols: instance
                        .protocols()
                        .iter()
                        .map(|range| ProtocolSupportResponse {
                            name: range.name().into(),
                            min_revision: range.min_revision(),
                            max_revision: range.max_revision(),
                        })
                        .collect(),
                    capabilities: instance
                        .capabilities()
                        .map(|capability| CapabilityResponse {
                            name: capability.name().into(),
                            version: capability.version().into(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}
