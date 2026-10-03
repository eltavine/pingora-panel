#![forbid(unsafe_code)]

//! `AuditPort` over `audit-service`, so the public API reads the audit trail
//! without owning it.

use async_trait::async_trait;
use panel_application::{
    AuditFilter, AuditPage, AuditPort, AuditRecord, AuditVerification, RequestScope,
};
use panel_contracts::audit::v1::{self as wire, audit_query_client::AuditQueryClient};
use panel_errors::{PanelError, Result};
use panel_service::{
    loopback_channel, propagate_trace, request_context, response_error, status_error,
    GrpcHealthCheck,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct AuditClient {
    channel: Channel,
}

impl AuditClient {
    /// A client that connects on first use to a plaintext loopback endpoint.
    pub fn connect_lazy(endpoint: impl Into<String>) -> Result<Self> {
        Ok(Self {
            channel: loopback_channel("audit service", endpoint, CONNECT_TIMEOUT, REQUEST_TIMEOUT)?,
        })
    }

    /// The channel owner authenticates externally supplied transports.
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    /// A readiness check against the service's standard gRPC health.
    pub fn health_check(&self) -> GrpcHealthCheck {
        GrpcHealthCheck::new(
            "audit-service",
            self.channel.clone(),
            wire::audit_query_server::SERVICE_NAME,
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

fn timestamp(time: SystemTime) -> prost_types::Timestamp {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    prost_types::Timestamp {
        seconds: i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(since.subsec_nanos()).unwrap_or_default(),
    }
}

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    let value = value?;
    let seconds = u64::try_from(value.seconds).ok()?;
    let nanos = u32::try_from(value.nanos).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

fn record(value: wire::AuditRecord) -> AuditRecord {
    AuditRecord {
        sequence: value.sequence,
        event_id: value.event_id,
        source: value.source,
        event_type: value.event_type,
        event_version: value.event_version,
        subject: value.subject,
        occurred_at: time(value.occurred_at),
        recorded_at: time(value.recorded_at),
        actor_type: value.actor_type,
        actor_id: value.actor_id,
        correlation_id: value.correlation_id,
        causation_id: value.causation_id,
        idempotency_key: value.idempotency_key,
        traceparent: value.traceparent,
        data: serde_json::from_slice(&value.data).unwrap_or(serde_json::Value::Null),
        hash: value.hash,
        previous_hash: value.previous_hash,
    }
}

#[async_trait]
impl AuditPort for AuditClient {
    async fn list(&self, scope: RequestScope, filter: AuditFilter) -> Result<AuditPage> {
        let request = wire::ListRequest {
            context: Some(request_context(&scope)),
            before: filter.before,
            limit: filter.limit,
            actor_id: filter.actor_id,
            event_type: filter.event_type,
            subject: filter.subject,
            correlation_id: filter.correlation_id,
            since: filter.since.map(timestamp),
            until: filter.until.map(timestamp),
        };
        let response = AuditQueryClient::new(self.channel.clone())
            .list(self.request(request, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(AuditPage {
            records: response.records.into_iter().map(record).collect(),
            next_before: response.next_before,
        })
    }

    async fn get(&self, scope: RequestScope, sequence: u64) -> Result<AuditRecord> {
        let request = wire::GetRequest {
            context: Some(request_context(&scope)),
            sequence,
        };
        let response = AuditQueryClient::new(self.channel.clone())
            .get(self.request(request, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response
            .record
            .map(record)
            .ok_or_else(|| PanelError::internal("the audit service returned no record"))
    }

    async fn verify(
        &self,
        scope: RequestScope,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<AuditVerification> {
        let request = wire::VerifyRequest {
            context: Some(request_context(&scope)),
            from,
            to,
        };
        let response = AuditQueryClient::new(self.channel.clone())
            .verify(self.request(request, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(AuditVerification {
            intact: response.intact,
            checked: response.checked,
            first_mismatch: response.first_mismatch,
            head_sequence: response.head_sequence,
            head_hash: response.head_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_plaintext_loopback_endpoints_connect_without_credentials() {
        assert!(AuditClient::connect_lazy("http://127.0.0.1:50064").is_ok());
        assert!(AuditClient::connect_lazy("http://[::1]:50064").is_ok());
        for endpoint in ["https://127.0.0.1:50064", "http://audit.internal:50064"] {
            assert!(AuditClient::connect_lazy(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn times_round_trip_through_the_wire() {
        let now = UNIX_EPOCH + Duration::new(1_800_000_000, 123_456_000);
        assert_eq!(time(Some(timestamp(now))), Some(now));
        assert_eq!(time(None), None);
    }
}
