use async_trait::async_trait;
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use std::time::Duration;
use tonic::{transport::Channel, Code};
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};

/// Passes while the gateway reports `SERVING` for the whole server.
pub struct GatewayHealthCheck {
    channel: Channel,
    timeout: Duration,
}

impl GatewayHealthCheck {
    pub(crate) fn new(channel: Channel, timeout: Duration) -> Self {
        Self { channel, timeout }
    }
}

#[async_trait]
impl HealthCheck for GatewayHealthCheck {
    fn component(&self) -> &str {
        "gatewayd"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Component
    }

    async fn check(&self) -> CheckOutcome {
        let mut request = tonic::Request::new(HealthCheckRequest {
            service: String::new(),
        });
        request.set_timeout(self.timeout);
        match HealthClient::new(self.channel.clone()).check(request).await {
            Ok(response) if response.get_ref().status() == ServingStatus::Serving => {
                CheckOutcome::pass()
            }
            Ok(_) => CheckOutcome::fail("not serving"),
            Err(status) if status.code() == Code::NotFound => {
                CheckOutcome::fail("health service unknown")
            }
            Err(_) => CheckOutcome::fail("unreachable"),
        }
    }
}
