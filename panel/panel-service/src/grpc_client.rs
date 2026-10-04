use async_trait::async_trait;
use panel_contracts::common::v1 as common;
use panel_errors::{ErrorCode, PanelError};
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use std::{net::IpAddr, time::Duration};
use tonic::{
    transport::{Channel, Endpoint},
    Code, Status,
};

/// A channel to the internal `service` at `endpoint`, connected on first
/// use. Without mutual TLS an endpoint must be a plaintext numeric loopback
/// address; the owner of a mutual TLS transport builds its channels itself.
pub fn loopback_channel(
    service: &str,
    endpoint: impl Into<String>,
    connect_timeout: Duration,
    timeout: Duration,
) -> Result<Channel, PanelError> {
    let endpoint = Endpoint::from_shared(endpoint.into()).map_err(|error| {
        PanelError::invalid_argument(format!("invalid {service} endpoint: {error}"))
    })?;
    let uri = endpoint.uri();
    let loopback = uri.scheme_str() == Some("http")
        && uri
            .host()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    if !loopback {
        return Err(PanelError::invalid_argument(format!(
            "the {service} endpoint must be a plaintext numeric loopback address \
             unless mutual TLS is enabled"
        )));
    }
    Ok(endpoint
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .connect_lazy())
}

/// The error a response reports, if it reports one.
pub fn response_error(error: Option<common::Error>) -> Result<(), PanelError> {
    error.map_or(Ok(()), |error| Err(error.into()))
}
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};

/// Maps a transport failure onto the stable error model. Only conditions
/// that may clear on their own are retryable.
pub fn status_error(status: Status) -> PanelError {
    let message = format!("RPC failed ({}): {}", status.code(), status.message());
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
        Code::Unavailable => PanelError::unavailable(message),
        Code::DataLoss => PanelError::corrupt_state(message),
        Code::Cancelled | Code::Internal | Code::Unknown | Code::Ok => {
            PanelError::internal(message)
        }
    };
    error.retryable(retryable)
}

/// Passes while a peer reports `SERVING` for `service` (empty for the whole
/// server) through the standard gRPC health service.
pub struct GrpcHealthCheck {
    component: String,
    channel: Channel,
    service: String,
    timeout: Duration,
}

impl GrpcHealthCheck {
    pub fn new(
        component: impl Into<String>,
        channel: Channel,
        service: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            component: component.into(),
            channel,
            service: service.into(),
            timeout,
        }
    }
}

#[async_trait]
impl HealthCheck for GrpcHealthCheck {
    fn component(&self) -> &str {
        &self.component
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Component
    }

    async fn check(&self) -> CheckOutcome {
        let mut request = tonic::Request::new(HealthCheckRequest {
            service: self.service.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_failures_map_to_stable_retryable_errors() {
        let error = status_error(Status::unavailable("connection refused"));
        assert_eq!(error.code.as_str(), ErrorCode::UNAVAILABLE);
        assert!(error.retryable);
        let error = status_error(Status::permission_denied("no"));
        assert_eq!(error.code.as_str(), ErrorCode::PERMISSION_DENIED);
        assert!(!error.retryable);
    }
}
