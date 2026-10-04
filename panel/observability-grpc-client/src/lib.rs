#![forbid(unsafe_code)]

//! `TrafficPort` over `observability-service`, so the public API reads what
//! the gateway served without querying Prometheus itself.

mod alerts;
mod host;
mod logs;

use async_trait::async_trait;
use panel_application::{
    CommandContext, DomainTraffic, Latency, RequestScope, RouteTraffic, StatusClasses,
    TrafficPoint, TrafficPort, TrafficQuery, TrafficSummary, UpstreamFailure, UpstreamTraffic,
};
use panel_contracts::{
    common::v1 as common,
    observability::v1::{self as wire, traffic_client::TrafficClient},
    PROTOCOL_VERSION,
};
use panel_errors::Result;
use panel_service::{
    loopback_channel, propagate_trace, request_context, response_error, status_error,
    GrpcHealthCheck,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct ObservabilityClient {
    channel: Channel,
}

impl ObservabilityClient {
    /// A client that connects on first use to a plaintext loopback endpoint.
    pub fn connect_lazy(endpoint: impl Into<String>) -> Result<Self> {
        Ok(Self {
            channel: loopback_channel(
                "observability service",
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
            "observability-service",
            self.channel.clone(),
            wire::traffic_server::SERVICE_NAME,
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

fn wire_scope(query: &TrafficQuery) -> wire::Scope {
    wire::Scope {
        site: query
            .site
            .as_ref()
            .map(|site| site.as_str().to_owned())
            .unwrap_or_default(),
        route: query
            .route
            .as_ref()
            .map(|route| route.as_str().to_owned())
            .unwrap_or_default(),
    }
}

fn duration(value: Duration) -> Option<prost_types::Duration> {
    prost_types::Duration::try_from(value).ok()
}

/// A command's request context, as services receive it.
fn command_context(context: &CommandContext) -> common::RequestContext {
    common::RequestContext {
        request_id: context.request_id().as_str().into(),
        correlation_id: context.correlation_id().as_str().into(),
        actor: context.actor().into(),
        deadline: context.deadline().as_str().into(),
        idempotency_key: context.idempotency_key().as_str().into(),
        schema_version: PROTOCOL_VERSION.into(),
        site_scope: None,
    }
}

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    let value = value?;
    let seconds = u64::try_from(value.seconds).ok()?;
    let nanos = u32::try_from(value.nanos).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

fn seconds(value: Option<f64>) -> Option<Duration> {
    value
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(Duration::from_secs_f64)
}

fn latency(value: Option<wire::Latency>) -> Latency {
    let value = value.unwrap_or_default();
    Latency {
        p50: seconds(value.p50),
        p90: seconds(value.p90),
        p95: seconds(value.p95),
        p99: seconds(value.p99),
    }
}

fn summary(value: wire::Summary) -> TrafficSummary {
    let statuses = value.statuses.unwrap_or_default();
    TrafficSummary {
        observed_at: time(value.observed_at),
        window: value
            .window
            .and_then(|window| Duration::try_from(window).ok())
            .unwrap_or_default(),
        requests: value.requests,
        requests_per_second: value.requests_per_second,
        statuses: StatusClasses {
            informational: statuses.informational,
            success: statuses.success,
            redirection: statuses.redirection,
            client_error: statuses.client_error,
            server_error: statuses.server_error,
        },
        latency: latency(value.latency),
        bytes_received: value.bytes_received,
        bytes_sent: value.bytes_sent,
        open_connections: value.open_connections,
        tls_handshakes: value.tls_handshakes,
        upstreams: value
            .upstreams
            .into_iter()
            .map(|upstream| UpstreamTraffic {
                upstream: upstream.upstream,
                requests: upstream.requests,
                error_ratio: upstream.error_ratio,
                latency: latency(upstream.latency),
                connection_reuse_ratio: upstream.connection_reuse_ratio,
            })
            .collect(),
        routes: value
            .routes
            .into_iter()
            .map(|route| RouteTraffic {
                site: route.site,
                route: route.route,
                requests: route.requests,
            })
            .collect(),
        upstream_failures: value
            .upstream_failures
            .into_iter()
            .filter_map(|failure| {
                Some(UpstreamFailure {
                    port: u16::try_from(failure.port).ok()?,
                    upstream: failure.upstream,
                    address: failure.address,
                    error_type: failure.error_type,
                    failures: failure.failures,
                })
            })
            .collect(),
        domains: value
            .domains
            .into_iter()
            .map(|domain| DomainTraffic {
                site: domain.site,
                domain: domain.domain,
                requests: domain.requests,
            })
            .collect(),
        revision: value.revision,
        activated_at: time(value.activated_at),
    }
}

#[async_trait]
impl TrafficPort for ObservabilityClient {
    async fn summary(&self, scope: RequestScope, query: TrafficQuery) -> Result<TrafficSummary> {
        let message = wire::SummaryRequest {
            context: Some(request_context(&scope)),
            scope: Some(wire_scope(&query)),
            window: query.window.and_then(duration),
        };
        let response = TrafficClient::new(self.channel.clone())
            .summary(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(summary(response.summary.unwrap_or_default()))
    }

    async fn series(
        &self,
        scope: RequestScope,
        query: TrafficQuery,
        step: Option<Duration>,
    ) -> Result<Vec<TrafficPoint>> {
        let message = wire::SeriesRequest {
            context: Some(request_context(&scope)),
            scope: Some(wire_scope(&query)),
            window: query.window.and_then(duration),
            step: step.and_then(duration),
        };
        let response = TrafficClient::new(self.channel.clone())
            .series(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response
            .points
            .into_iter()
            .filter_map(|point| {
                Some(TrafficPoint {
                    at: time(point.at)?,
                    requests_per_second: point.requests_per_second,
                    server_errors_per_second: point.server_errors_per_second,
                    p95: seconds(point.p95),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_plaintext_loopback_endpoints_connect_without_credentials() {
        assert!(ObservabilityClient::connect_lazy("http://127.0.0.1:50063").is_ok());
        for endpoint in [
            "https://127.0.0.1:50063",
            "http://observability.internal:50063",
        ] {
            assert!(
                ObservabilityClient::connect_lazy(endpoint).is_err(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn summaries_read_seconds_as_durations() {
        let read = summary(wire::Summary {
            window: duration(Duration::from_secs(3600)),
            latency: Some(wire::Latency {
                p50: Some(0.25),
                p99: Some(f64::NAN),
                ..wire::Latency::default()
            }),
            revision: Some(7),
            ..wire::Summary::default()
        });
        assert_eq!(read.window, Duration::from_secs(3600));
        assert_eq!(read.latency.p50, Some(Duration::from_millis(250)));
        assert_eq!(read.latency.p99, None);
        assert_eq!(read.revision, Some(7));
    }
}
