#![forbid(unsafe_code)]

//! `CertificatePort` over `automation-service`, so the public API manages
//! the certificate inventory without holding private keys.

use async_trait::async_trait;
use panel_application::{
    CertificateChange, CertificateOutput, CertificatePort, CertificateRead, CommandContext,
    RequestScope,
};
use panel_contracts::{
    automation::v1::{self as wire, certificates_client::CertificatesClient},
    common::v1 as common,
    PROTOCOL_VERSION,
};
use panel_errors::{PanelError, Result};
use panel_service::{propagate_trace, status_error, GrpcHealthCheck};
use std::{net::IpAddr, time::Duration};
use tonic::transport::{Channel, Endpoint};
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct AutomationClient {
    channel: Channel,
}

impl AutomationClient {
    /// A client that connects on first use to a plaintext loopback endpoint.
    pub fn connect_lazy(endpoint: impl Into<String>) -> Result<Self> {
        let endpoint = Endpoint::from_shared(endpoint.into()).map_err(|error| {
            PanelError::invalid_argument(format!("invalid automation service endpoint: {error}"))
        })?;
        let uri = endpoint.uri();
        let loopback = uri.scheme_str() == Some("http")
            && uri
                .host()
                .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
                .and_then(|host| host.parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
        if !loopback {
            return Err(PanelError::invalid_argument(
                "the automation service endpoint must be a plaintext numeric loopback address \
                 unless mutual TLS is enabled",
            ));
        }
        Ok(Self {
            channel: endpoint
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .connect_lazy(),
        })
    }

    /// The channel owner authenticates externally supplied transports.
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    /// A readiness check against the service's standard gRPC health.
    pub fn health_check(&self) -> GrpcHealthCheck {
        GrpcHealthCheck::new(
            "automation-service",
            self.channel.clone(),
            wire::certificates_server::SERVICE_NAME,
            REQUEST_TIMEOUT,
        )
    }

    fn request<T>(&self, message: T, scope: &RequestScope) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        request.set_timeout(REQUEST_TIMEOUT);
        propagate_trace(request.metadata_mut(), scope.trace_context());
        request
    }
}

fn checked(error: Option<common::Error>) -> Result<()> {
    error.map_or(Ok(()), |error| Err(error.into()))
}

fn etag(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[async_trait]
impl CertificatePort for AutomationClient {
    async fn read(&self, scope: RequestScope, read: CertificateRead) -> Result<CertificateOutput> {
        let message = wire::ReadRequest {
            context: Some(common::RequestContext {
                request_id: scope.request_id().as_str().into(),
                correlation_id: scope.correlation_id().as_str().into(),
                schema_version: PROTOCOL_VERSION.into(),
                ..common::RequestContext::default()
            }),
            operation: read.operation,
            resource: read.resource,
            parameters: read.parameters,
        };
        let response = CertificatesClient::new(self.channel.clone())
            .read(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        checked(response.error)?;
        Ok(CertificateOutput {
            content: response.content,
            etag: etag(response.etag),
        })
    }

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput> {
        let scope = context.scope();
        let content = Zeroizing::new(change.content);
        let message = wire::ChangeRequest {
            context: Some(common::RequestContext {
                request_id: context.request_id().as_str().into(),
                correlation_id: context.correlation_id().as_str().into(),
                actor: context.actor().into(),
                deadline: context.deadline().as_str().into(),
                idempotency_key: context.idempotency_key().as_str().into(),
                schema_version: PROTOCOL_VERSION.into(),
            }),
            operation: change.operation,
            resource: change.resource,
            if_match: change.if_match.unwrap_or_default(),
            content: content.to_vec(),
        };
        let response = CertificatesClient::new(self.channel.clone())
            .change(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        checked(response.error)?;
        Ok(CertificateOutput {
            content: response.content,
            etag: etag(response.etag),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plaintext_endpoints_stay_on_loopback() {
        assert!(AutomationClient::connect_lazy("http://127.0.0.1:50062").is_ok());
        assert!(AutomationClient::connect_lazy("http://[::1]:50062").is_ok());
        for remote in [
            "http://10.0.0.5:50062",
            "https://127.0.0.1:50062",
            "http://localhost:50062",
        ] {
            assert!(AutomationClient::connect_lazy(remote).is_err(), "{remote}");
        }
    }
}
