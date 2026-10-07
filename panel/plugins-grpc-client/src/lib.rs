#![forbid(unsafe_code)]

//! `PluginsPort` over `plugins-service`, so the public API manages plugins
//! without running them or holding the secrets their settings name.

use async_trait::async_trait;
use panel_application::{CommandContext, RequestScope};
use panel_contracts::plugins::v1::{self as wire, plugins_client::PluginsClient as Client};
use panel_errors::Result;
use panel_plugin_api::{PluginChange, PluginOutput, PluginQuery, PluginsPort};
use panel_service::{
    command_context, loopback_channel, propagate_trace, request_context, response_error,
    status_error, GrpcHealthCheck,
};
use std::time::Duration;
use tonic::transport::Channel;
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Long enough for a plugin to start, shake hands and apply its settings.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct PluginsClient {
    channel: Channel,
}

impl PluginsClient {
    /// A client that connects on first use to a plaintext loopback endpoint.
    pub fn connect_lazy(endpoint: impl Into<String>) -> Result<Self> {
        Ok(Self {
            channel: loopback_channel(
                "plugins service",
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

    /// The channel, for the ports plugins provide through the service.
    pub fn channel(&self) -> Channel {
        self.channel.clone()
    }

    /// A readiness check against the service's standard gRPC health.
    pub fn health_check(&self) -> GrpcHealthCheck {
        GrpcHealthCheck::new(
            "plugins-service",
            self.channel.clone(),
            wire::plugins_server::SERVICE_NAME,
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
impl PluginsPort for PluginsClient {
    async fn read(&self, scope: RequestScope, query: PluginQuery) -> Result<PluginOutput> {
        let message = wire::ReadRequest {
            context: Some(request_context(&scope)),
            query: serde_json::to_vec(&query).expect("plugin queries serialize"),
        };
        let response = Client::new(self.channel.clone())
            .read(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(PluginOutput {
            content: response.content,
            etag: etag(response.etag),
        })
    }

    async fn change(&self, context: CommandContext, change: PluginChange) -> Result<PluginOutput> {
        let scope = context.scope();
        let command =
            Zeroizing::new(serde_json::to_vec(&change.command).expect("plugin commands serialize"));
        let message = wire::ChangeRequest {
            context: Some(command_context(&context)),
            if_match: change.if_match.unwrap_or_default(),
            command: command.to_vec(),
        };
        let response = Client::new(self.channel.clone())
            .change(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(PluginOutput {
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
        assert!(PluginsClient::connect_lazy("http://127.0.0.1:50066").is_ok());
        assert!(PluginsClient::connect_lazy("http://[::1]:50066").is_ok());
        for remote in [
            "http://10.0.0.5:50066",
            "https://127.0.0.1:50066",
            "http://localhost:50066",
        ] {
            assert!(PluginsClient::connect_lazy(remote).is_err(), "{remote}");
        }
    }
}
