use panel_contracts::platform::v1::{
    self as wire,
    service_info_client::ServiceInfoClient,
    service_info_server::{ServiceInfo, ServiceInfoServer},
};
use panel_errors::{PanelError, Result};
use panel_health::{HealthStatus, HealthWatch};
use panel_platform::{NegotiatedProtocol, ProtocolRange, ServiceDescriptor};
use panel_platform_codec::{decode_descriptor, encode_descriptor};
use std::time::Duration;
use tonic::{transport::Channel, Code, Request, Response, Status};
use tonic_health::{server::HealthReporter, ServingStatus};

/// Mirrors readiness into the standard gRPC health service for `services`
/// (include `""` for the whole server): `SERVING` unless readiness fails.
/// Runs until the health monitor stops.
pub async fn publish_grpc_health(
    mut health: HealthWatch,
    reporter: HealthReporter,
    services: Vec<String>,
) {
    loop {
        let status = if health.current().status() == HealthStatus::Fail {
            ServingStatus::NotServing
        } else {
            ServingStatus::Serving
        };
        for service in &services {
            reporter.set_service_status(service, status).await;
        }
        if !health.changed().await {
            return;
        }
    }
}

/// Answers `ServiceInfo.Describe` with this instance's descriptor.
#[derive(Clone)]
pub struct ServiceInfoService {
    descriptor: wire::ServiceDescriptor,
}

impl ServiceInfoService {
    pub fn new(descriptor: &ServiceDescriptor) -> Self {
        Self {
            descriptor: encode_descriptor(descriptor),
        }
    }

    pub fn into_server(self) -> ServiceInfoServer<Self> {
        ServiceInfoServer::new(self)
    }
}

#[tonic::async_trait]
impl ServiceInfo for ServiceInfoService {
    async fn describe(
        &self,
        _request: Request<wire::DescribeRequest>,
    ) -> std::result::Result<Response<wire::DescribeResponse>, Status> {
        Ok(Response::new(wire::DescribeResponse {
            descriptor: Some(self.descriptor.clone()),
        }))
    }
}

/// Asks a peer to describe itself.
pub async fn describe_peer(channel: Channel, timeout: Duration) -> Result<ServiceDescriptor> {
    let mut request = Request::new(wire::DescribeRequest {});
    request.set_timeout(timeout);
    let response = ServiceInfoClient::new(channel)
        .describe(request)
        .await
        .map_err(|status| match status.code() {
            Code::Unimplemented => {
                PanelError::unsupported_capability("peer does not describe itself")
            }
            _ => PanelError::unavailable(format!("peer description failed: {}", status.code())),
        })?
        .into_inner();
    decode_descriptor(
        response
            .descriptor
            .ok_or_else(|| PanelError::invalid_argument("peer description has no descriptor"))?,
    )
}

/// Describes a peer and negotiates the highest revision of `local` it
/// speaks, failing fast when the peer speaks none.
pub async fn negotiate_with_peer(
    channel: Channel,
    local: &ProtocolRange,
    timeout: Duration,
) -> Result<(ServiceDescriptor, NegotiatedProtocol)> {
    let descriptor = describe_peer(channel, timeout).await?;
    let negotiated = local.negotiate_with_any(descriptor.protocols())?;
    Ok((descriptor, negotiated))
}
