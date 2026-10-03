#![forbid(unsafe_code)]

//! `GatewayUseCases` and `ConfigurationPort` over `config-service`, so the
//! public API composes publication and configuration without owning them.

mod configuration;

use async_trait::async_trait;
use config_proto_codec as codec;
use gateway_proto_codec::encode_hash;
use panel_application::{
    AbortOutcome, ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayStatus,
    GatewayUseCases, IdempotencyKey, IdempotencyLookup, PreparedDeployment, RequestId,
    RequestScope, TraceContext,
};
use panel_contracts::config::v1::{self as wire, publication_client::PublicationClient};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_service::{propagate_trace, status_error, GrpcHealthCheck};
use std::{net::IpAddr, time::Duration};
use tonic::transport::{Channel, Endpoint};
use uuid::Uuid;

pub(crate) const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const CLIENT: &str = "config-grpc-client";

/// Connection policy for the publication client.
#[derive(Clone, Copy, Debug)]
pub struct ConfigClientConfig {
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl Default for ConfigClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl ConfigClientConfig {
    pub fn with_request_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(PanelError::invalid_argument(
                "request timeout must be non-zero",
            ));
        }
        self.request_timeout = timeout;
        Ok(self)
    }
}

#[derive(Clone)]
pub struct ConfigPublicationClient {
    channel: Channel,
    config: ConfigClientConfig,
}

impl ConfigPublicationClient {
    /// A client that connects on first use to a plaintext loopback endpoint.
    pub fn connect_lazy(endpoint: impl Into<String>, config: ConfigClientConfig) -> Result<Self> {
        let endpoint = Endpoint::from_shared(endpoint.into()).map_err(|error| {
            PanelError::invalid_argument(format!("invalid config service endpoint: {error}"))
        })?;
        require_plaintext_loopback(&endpoint)?;
        let channel = endpoint
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .connect_lazy();
        Ok(Self { channel, config })
    }

    /// The channel owner authenticates externally supplied transports.
    pub fn from_channel(channel: Channel, config: ConfigClientConfig) -> Self {
        Self { channel, config }
    }

    /// A readiness check against the service's standard gRPC health.
    pub fn health_check(&self) -> GrpcHealthCheck {
        GrpcHealthCheck::new(
            "config-service",
            self.channel.clone(),
            wire::publication_server::SERVICE_NAME,
            self.config.request_timeout,
        )
    }

    fn client(&self) -> PublicationClient<Channel> {
        PublicationClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
    }

    pub(crate) fn request<T>(&self, message: T, trace: Option<&TraceContext>) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        request.set_timeout(self.config.request_timeout);
        propagate_trace(request.metadata_mut(), trace);
        request
    }
}

/// A scope for calls made without a caller, which start a correlation.
fn standalone_scope(operation: &str) -> Result<RequestScope> {
    RequestId::new(format!("{CLIENT}-{operation}-{}", Uuid::now_v7())).map(RequestScope::new)
}

fn require_plaintext_loopback(endpoint: &Endpoint) -> Result<()> {
    let uri = endpoint.uri();
    let loopback = uri.scheme_str() == Some("http")
        && uri
            .host()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(PanelError::invalid_argument(
            "the config service endpoint must be a plaintext numeric loopback address \
             until internal transports are authenticated",
        ))
    }
}

#[async_trait]
impl GatewayUseCases for ConfigPublicationClient {
    async fn validate(&self, document: ConfigDocument) -> Result<ValidationReport> {
        self.validate_with_scope(standalone_scope("validate")?, document)
            .await
    }

    async fn validate_with_scope(
        &self,
        scope: RequestScope,
        document: ConfigDocument,
    ) -> Result<ValidationReport> {
        let request = wire::ValidateRequest {
            context: Some(codec::encode_scope(&scope)),
            document: Some(codec::encode_document(&document)),
        };
        let response = self
            .client()
            .validate(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        codec::decode_report(response.report)
    }

    async fn prepare(
        &self,
        context: CommandContext,
        document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        let request = wire::PrepareRequest {
            context: Some(codec::encode_command(&context)),
            document: Some(codec::encode_document(&document)),
        };
        let response = self
            .client()
            .prepare(self.request(request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        codec::decode_prepared(response.deployment)
    }

    async fn activate(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let request = wire::ActivateRequest {
            context: Some(codec::encode_command(&context)),
            prepare_token,
            expected_active_hash: expected_active_hash.as_ref().map(encode_hash),
        };
        let response = self
            .client()
            .activate(self.request(request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        codec::decode_activated(response.deployment)
    }

    async fn abort(&self, context: CommandContext, prepare_token: String) -> Result<AbortOutcome> {
        let request = wire::AbortRequest {
            context: Some(codec::encode_command(&context)),
            prepare_token,
        };
        let response = self
            .client()
            .abort(self.request(request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        Ok(codec::decode_abort(response.aborted))
    }

    async fn status(&self) -> Result<GatewayStatus> {
        self.status_with_scope(standalone_scope("status")?).await
    }

    async fn status_with_scope(&self, scope: RequestScope) -> Result<GatewayStatus> {
        let request = wire::GetGatewayStatusRequest {
            context: Some(codec::encode_scope(&scope)),
        };
        let response = self
            .client()
            .get_gateway_status(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        codec::decode_status(response.status)
    }

    async fn activation_receipt(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        let scope = standalone_scope("receipt")?;
        let request = wire::GetActivationReceiptRequest {
            context: Some(codec::encode_scope(&scope)),
            idempotency_key: key.as_str().into(),
        };
        let response = self
            .client()
            .get_activation_receipt(self.request(request, None))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        codec::decode_lookup(response.state, response.receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_plaintext_loopback_endpoints_are_accepted() {
        for endpoint in ["http://127.0.0.1:50061", "http://[::1]:50061"] {
            assert!(
                ConfigPublicationClient::connect_lazy(endpoint, ConfigClientConfig::default())
                    .is_ok(),
                "{endpoint}"
            );
        }
        for endpoint in [
            "http://192.0.2.1:50061",
            "http://config-service:50061",
            "https://127.0.0.1:50061",
        ] {
            assert!(
                ConfigPublicationClient::connect_lazy(endpoint, ConfigClientConfig::default())
                    .is_err(),
                "{endpoint}"
            );
        }
    }
}
