#![forbid(unsafe_code)]

//! `CertificatePort` over `automation-service`, so the public API manages
//! the certificate inventory without holding private keys.

use async_trait::async_trait;
use panel_application::{CommandContext, RequestScope};
use panel_certificate_api::{
    CertificateChange, CertificateOutput, CertificatePort, CertificateQuery,
};
use panel_contracts::automation::v1::{self as wire, certificates_client::CertificatesClient};
use panel_errors::Result;
use panel_service::{
    command_context, loopback_channel, propagate_trace, request_context, response_error,
    status_error, GrpcHealthCheck,
};
use std::time::Duration;
use tonic::transport::Channel;
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
        Ok(Self {
            channel: loopback_channel(
                "automation service",
                endpoint,
                CONNECT_TIMEOUT,
                REQUEST_TIMEOUT,
            )?,
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

fn etag(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[async_trait]
impl CertificatePort for AutomationClient {
    async fn read(
        &self,
        scope: RequestScope,
        query: CertificateQuery,
    ) -> Result<CertificateOutput> {
        let message = wire::ReadRequest {
            context: Some(request_context(&scope)),
            query: serde_json::to_vec(&query).expect("certificate queries serialize"),
            ..wire::ReadRequest::default()
        };
        let response = CertificatesClient::new(self.channel.clone())
            .read(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
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
        let command = Zeroizing::new(
            serde_json::to_vec(&change.command).expect("certificate commands serialize"),
        );
        let message = wire::ChangeRequest {
            context: Some(command_context(&context)),
            if_match: change.if_match.unwrap_or_default(),
            command: command.to_vec(),
            ..wire::ChangeRequest::default()
        };
        let response = CertificatesClient::new(self.channel.clone())
            .change(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
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
