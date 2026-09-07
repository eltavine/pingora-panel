#![forbid(unsafe_code)]

//! Tonic client adapter for the application-owned `GatewayPort`.
//!
//! Generated Proto and Tonic types are intentionally confined to this crate;
//! callers depend only on `panel-application` values and ports.

use async_trait::async_trait;
use gateway_grpc::encode_snapshot;
use panel_application::{
    ActivatedDeployment, CommandContext, ContentHash, GatewayPort, PreparedDeployment,
};
use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::RevisionId;
use panel_errors::{
    Diagnostic, DiagnosticSeverity, ErrorCode, PanelError, Result, ValidationReport,
};
use panel_ir::RuntimeSnapshot;
use std::time::Duration;
use tonic::{
    transport::{Channel, Endpoint},
    Code, Status,
};

const CONTEXT_SCHEMA_VERSION: &str = "v1";

pub struct GatewayGrpcClient {
    channel: Channel,
    max_message_bytes: usize,
    request_timeout: Duration,
}

/// Injectable connection and message policies keep operational limits out of
/// the application port and make upgrades additive.
#[derive(Clone, Debug)]
pub struct GatewayGrpcClientConfig {
    connect_timeout: Duration,
    request_timeout: Duration,
    max_message_bytes: usize,
}

impl Default for GatewayGrpcClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            max_message_bytes: 16 * 1024 * 1024,
        }
    }
}

impl GatewayGrpcClientConfig {
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_max_message_bytes(mut self, max_message_bytes: usize) -> Self {
        self.max_message_bytes = max_message_bytes;
        self
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }

    pub fn validate(&self) -> Result<()> {
        if self.connect_timeout.is_zero() || self.request_timeout.is_zero() {
            return Err(PanelError::invalid_argument(
                "gateway timeouts must be greater than zero",
            ));
        }
        if self.max_message_bytes == 0 {
            return Err(PanelError::invalid_argument(
                "gateway max_message_bytes must be greater than zero",
            ));
        }
        Ok(())
    }
}

impl GatewayGrpcClient {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::connect_with_config(endpoint, GatewayGrpcClientConfig::default()).await
    }

    pub async fn connect_with_config(
        endpoint: impl Into<String>,
        config: GatewayGrpcClientConfig,
    ) -> Result<Self> {
        config.validate()?;
        let endpoint = Endpoint::from_shared(endpoint.into())
            .map_err(|error| {
                PanelError::invalid_argument(format!("invalid gateway endpoint: {error}"))
            })?
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout);
        let channel = endpoint.connect().await.map_err(|error| {
            PanelError::storage_unavailable(format!("gateway connect failed: {error}"))
                .with_source(error)
        })?;
        Ok(Self {
            channel,
            max_message_bytes: config.max_message_bytes,
            request_timeout: config.request_timeout,
        })
    }

    pub fn from_channel(channel: Channel) -> Self {
        let config = GatewayGrpcClientConfig::default();
        Self {
            channel,
            max_message_bytes: config.max_message_bytes,
            request_timeout: config.request_timeout,
        }
    }

    pub fn from_channel_with_config(
        channel: Channel,
        config: GatewayGrpcClientConfig,
    ) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            channel,
            max_message_bytes: config.max_message_bytes,
            request_timeout: config.request_timeout,
        })
    }

    fn client(&self) -> wire::gateway_engine_client::GatewayEngineClient<Channel> {
        wire::gateway_engine_client::GatewayEngineClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_message_bytes)
            .max_encoding_message_size(self.max_message_bytes)
    }

    fn request<T>(&self, value: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(value);
        request.set_timeout(self.request_timeout);
        request
    }
}

fn context(value: &CommandContext) -> common::RequestContext {
    common::RequestContext {
        request_id: value.request_id().as_str().into(),
        correlation_id: value.correlation_id().as_str().into(),
        actor: value.actor().into(),
        deadline: value.deadline().as_str().into(),
        idempotency_key: value.idempotency_key().as_str().into(),
        schema_version: CONTEXT_SCHEMA_VERSION.into(),
    }
}

fn diagnostic(value: common::Diagnostic) -> Diagnostic {
    let severity = match common::DiagnosticSeverity::try_from(value.severity)
        .unwrap_or(common::DiagnosticSeverity::Error)
    {
        common::DiagnosticSeverity::Info => DiagnosticSeverity::Info,
        common::DiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Error,
    };
    Diagnostic {
        code: ErrorCode::new(value.code),
        severity,
        message: value.message,
        source_span: (!value.source_span.is_empty()).then_some(value.source_span),
        resource_id: (!value.resource_id.is_empty()).then_some(value.resource_id),
        help: (!value.help.is_empty()).then_some(value.help),
    }
}

fn error(value: common::Error) -> PanelError {
    PanelError::new(value.code, value.message)
        .retryable(value.retryable)
        .with_diagnostics(value.diagnostics.into_iter().map(diagnostic).collect())
}

fn response_error(value: Option<common::Error>) -> Result<()> {
    value.map_or(Ok(()), |value| Err(error(value)))
}

fn status_error(status: Status) -> PanelError {
    let message = format!(
        "gateway RPC failed ({}): {}",
        status.code(),
        status.message()
    );
    let retryable = matches!(
        status.code(),
        Code::Unavailable | Code::ResourceExhausted | Code::DeadlineExceeded
    );
    let error = match status.code() {
        Code::InvalidArgument | Code::OutOfRange => {
            PanelError::new(ErrorCode::INVALID_ARGUMENT, message)
        }
        Code::DeadlineExceeded => PanelError::deadline_exceeded(message),
        Code::NotFound => PanelError::not_found(message),
        Code::AlreadyExists | Code::Aborted => PanelError::conflict(message),
        Code::PermissionDenied => PanelError::permission_denied(message),
        Code::Unauthenticated => PanelError::unauthenticated(message),
        Code::ResourceExhausted => PanelError::resource_exhausted(message),
        Code::FailedPrecondition => PanelError::precondition_failed(message),
        Code::Unimplemented => PanelError::unsupported_capability(message),
        Code::Unavailable => PanelError::storage_unavailable(message),
        Code::DataLoss => PanelError::corrupt_state(message),
        Code::Cancelled | Code::Internal | Code::Unknown | Code::Ok => {
            PanelError::internal(message)
        }
    };
    error.retryable(retryable).with_source(status)
}

fn hash(value: Option<common::ContentHash>) -> Result<ContentHash> {
    let value =
        value.ok_or_else(|| PanelError::invalid_argument("gateway response hash is missing"))?;
    if value.algorithm != "sha256" {
        return Err(PanelError::invalid_argument(
            "unsupported gateway hash algorithm",
        ));
    }
    ContentHash::from_hex(value.value)
        .map_err(|error| PanelError::invalid_argument(error.to_string()))
}

#[async_trait]
impl GatewayPort for GatewayGrpcClient {
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        let request = wire::ValidateRequest {
            context: None,
            snapshot: Some(encode_snapshot(&snapshot)),
        };
        let mut client = self.client();
        let response = client
            .validate(self.request(request))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ValidationReport {
            valid: response.valid,
            diagnostics: response.diagnostics.into_iter().map(diagnostic).collect(),
        })
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment> {
        let request = wire::PrepareRequest {
            context: None,
            snapshot: Some(encode_snapshot(&snapshot)),
        };
        let mut client = self.client();
        let response = client
            .prepare(self.request(request))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        PreparedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.content_hash)?,
            response.prepare_token,
        )
    }

    async fn activate(
        &self,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let request = wire::ActivateRequest {
            context: None,
            prepare_token,
            expected_active_hash: expected_active_hash.map(|value| common::ContentHash {
                algorithm: "sha256".into(),
                value: value.as_str().into(),
            }),
        };
        let mut client = self.client();
        let response = client
            .activate(self.request(request))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ActivatedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.active_hash)?,
            response
                .previous_active_hash
                .map(|value| hash(Some(value)))
                .transpose()?,
        ))
    }

    async fn prepare_with_context(
        &self,
        context_value: CommandContext,
        snapshot: RuntimeSnapshot,
    ) -> Result<PreparedDeployment> {
        let request = wire::PrepareRequest {
            context: Some(context(&context_value)),
            snapshot: Some(encode_snapshot(&snapshot)),
        };
        let mut client = self.client();
        let response = client
            .prepare(self.request(request))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        PreparedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.content_hash)?,
            response.prepare_token,
        )
    }

    async fn activate_with_context(
        &self,
        context_value: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let request = wire::ActivateRequest {
            context: Some(context(&context_value)),
            prepare_token,
            expected_active_hash: expected_active_hash.map(|value| common::ContentHash {
                algorithm: "sha256".into(),
                value: value.as_str().into(),
            }),
        };
        let mut client = self.client();
        let response = client
            .activate(self.request(request))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ActivatedDeployment::new(
            RevisionId::new(response.revision_id),
            hash(response.active_hash)?,
            response
                .previous_active_hash
                .map(|value| hash(Some(value)))
                .transpose()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_unbounded_or_zero_limits() {
        let config = GatewayGrpcClientConfig::default().with_max_message_bytes(0);
        assert_eq!(
            config.validate().unwrap_err().code.as_str(),
            ErrorCode::INVALID_ARGUMENT
        );

        let config = GatewayGrpcClientConfig::default().with_request_timeout(Duration::ZERO);
        assert_eq!(
            config.validate().unwrap_err().code.as_str(),
            ErrorCode::INVALID_ARGUMENT
        );
    }

    #[test]
    fn transport_status_maps_to_stable_error_and_retry_policy() {
        let error = status_error(Status::deadline_exceeded("upstream timed out"));
        assert_eq!(error.code.as_str(), ErrorCode::DEADLINE_EXCEEDED);
        assert!(error.retryable);
        assert!(error.message.contains("upstream timed out"));

        let error = status_error(Status::permission_denied("no access"));
        assert_eq!(error.code.as_str(), ErrorCode::PERMISSION_DENIED);
        assert!(!error.retryable);
    }
}
