use crate::{AggregateRef, EventPayload, EventType, Principal, ServiceName};
use chrono::{DateTime, Utc};
use panel_context::{IdempotencyKey, RequestId, RequestScope, TraceContext};
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, num::NonZeroU32, str::FromStr};
use uuid::Uuid;

/// CloudEvents specification version produced and accepted.
pub const CLOUDEVENTS_SPEC_VERSION: &str = "1.0";
/// Reverse-DNS prefix of every CloudEvents `type` defined by this product.
pub const EVENT_TYPE_NAMESPACE: &str = "io.github.eltavine.pingora-panel";
/// Path prefix of the CloudEvents `source` URI-reference of a service.
pub const EVENT_SOURCE_PREFIX: &str = "/pingora-panel/";

/// Globally unique, time-ordered event identity (UUIDv7), used as the
/// CloudEvents `id`.
///
/// Consumers deduplicate on this value, so it must never be reused. The
/// byte representation is exposed instead of a UUID library type so adapters
/// remain free to choose their own UUID implementation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventId(Uuid);

impl EventId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Result<Self> {
        let value = Uuid::from_bytes(bytes);
        if value.is_nil() || value.is_max() {
            return Err(PanelError::invalid_argument(
                "event id must not be the nil or max UUID",
            ));
        }
        Ok(Self(value))
    }

    pub fn parse(value: &str) -> Result<Self> {
        let parsed = Uuid::try_parse(value)
            .map_err(|_| PanelError::invalid_argument("event id must be a UUID"))?;
        Self::from_bytes(parsed.into_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

impl FromStr for EventId {
    type Err = PanelError;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl Serialize for EventId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Major version of an event's data schema. Following the CloudEvents
/// versioning guidance, an incompatible data change produces a new `type`
/// through this version, while compatible changes keep it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventVersion(NonZeroU32);

impl EventVersion {
    pub const V1: Self = Self(NonZeroU32::MIN);

    pub fn new(value: u32) -> Result<Self> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or_else(|| PanelError::invalid_argument("event version must be at least 1"))
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Builds the CloudEvents `type`, for example
/// `io.github.eltavine.pingora-panel.config.revision.activated.v1`.
pub fn qualified_event_type(event_type: &EventType, version: EventVersion) -> String {
    format!("{EVENT_TYPE_NAMESPACE}.{event_type}.v{}", version.get())
}

/// Parses a CloudEvents `type` produced by [`qualified_event_type`].
pub fn parse_qualified_event_type(value: &str) -> Result<(EventType, EventVersion)> {
    let unsupported =
        || PanelError::invalid_argument(format!("event type {value} is not a product event type"));
    let name = value
        .strip_prefix(EVENT_TYPE_NAMESPACE)
        .and_then(|rest| rest.strip_prefix('.'))
        .ok_or_else(unsupported)?;
    let (name, version) = name.rsplit_once(".v").ok_or_else(unsupported)?;
    if version.is_empty()
        || version.starts_with('0')
        || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(unsupported());
    }
    let version = version.parse::<u32>().map_err(|_| unsupported())?;
    Ok((EventType::new(name)?, EventVersion::new(version)?))
}

/// Builds the CloudEvents `source` URI-reference of a service.
pub fn event_source(producer: &ServiceName) -> String {
    format!("{EVENT_SOURCE_PREFIX}{producer}")
}

/// Parses a CloudEvents `source` produced by [`event_source`].
pub fn parse_event_source(value: &str) -> Result<ServiceName> {
    value
        .strip_prefix(EVENT_SOURCE_PREFIX)
        .ok_or_else(|| {
            PanelError::invalid_argument(format!("event source {value} is not a product service"))
        })
        .and_then(ServiceName::new)
}

/// What happened: the producer-independent part of a new event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventDraft {
    event_type: EventType,
    event_version: EventVersion,
    aggregate: AggregateRef,
    payload: EventPayload,
}

impl EventDraft {
    pub fn new(
        event_type: EventType,
        event_version: EventVersion,
        aggregate: AggregateRef,
        payload: EventPayload,
    ) -> Self {
        Self {
            event_type,
            event_version,
            aggregate,
            payload,
        }
    }
}

/// Who produced an event and why.
///
/// Follows the CloudEvents Correlation extension: the correlation identity
/// is generated at the system entry point and inherited unchanged through a
/// whole causal chain; the causation identity always names the direct cause.
/// Per the Distributed Tracing extension, the trace context is that of the
/// trace which started the chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventOrigin {
    producer: ServiceName,
    correlation_id: RequestId,
    causation_id: RequestId,
    principal: Principal,
    idempotency_key: Option<IdempotencyKey>,
    trace_context: Option<TraceContext>,
}

impl EventOrigin {
    /// An event caused directly by a request or command.
    pub fn request(
        producer: ServiceName,
        request_id: &RequestId,
        correlation_id: &RequestId,
        principal: Principal,
    ) -> Self {
        Self {
            producer,
            correlation_id: correlation_id.clone(),
            causation_id: request_id.clone(),
            principal,
            idempotency_key: None,
            trace_context: None,
        }
    }

    /// An event caused directly by the request described by `scope`; its
    /// correlation identity and trace context propagate.
    pub fn scoped(producer: ServiceName, scope: &RequestScope, principal: Principal) -> Self {
        Self {
            producer,
            correlation_id: scope.correlation_id().clone(),
            causation_id: scope.request_id().clone(),
            principal,
            idempotency_key: None,
            trace_context: scope.trace_context().cloned(),
        }
    }

    /// An event caused by `parent`. Correlation, principal, idempotency key
    /// and trace context propagate; the parent's event ID becomes the cause.
    pub fn caused_by(producer: ServiceName, parent: &EventEnvelope) -> Self {
        Self {
            producer,
            correlation_id: parent.correlation_id.clone(),
            causation_id: parent.event_id_as_request_id(),
            principal: parent.principal.clone(),
            idempotency_key: parent.idempotency_key.clone(),
            trace_context: parent.trace_context.clone(),
        }
    }

    pub fn with_idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    pub fn with_principal(mut self, principal: Principal) -> Self {
        self.principal = principal;
        self
    }

    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }
}

/// Every field of an envelope. Wire codecs construct and destructure this
/// exhaustively so a new envelope field fails to compile in each codec
/// instead of being silently dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventEnvelopeParts {
    pub event_id: EventId,
    pub event_type: EventType,
    pub event_version: EventVersion,
    pub occurred_at: DateTime<Utc>,
    pub producer: ServiceName,
    pub aggregate: AggregateRef,
    pub correlation_id: RequestId,
    pub causation_id: RequestId,
    pub principal: Principal,
    pub idempotency_key: Option<IdempotencyKey>,
    pub trace_context: Option<TraceContext>,
    pub payload: EventPayload,
}

/// An immutable, attributable domain event: a CloudEvents 1.0 event using
/// the Correlation, Auth Context and Distributed Tracing extensions plus the
/// product `idempotencykey` extension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventEnvelope {
    event_id: EventId,
    event_type: EventType,
    event_version: EventVersion,
    occurred_at: DateTime<Utc>,
    producer: ServiceName,
    aggregate: AggregateRef,
    correlation_id: RequestId,
    causation_id: RequestId,
    principal: Principal,
    idempotency_key: Option<IdempotencyKey>,
    trace_context: Option<TraceContext>,
    payload: EventPayload,
}

impl EventEnvelope {
    /// Creates a new event with a fresh identity.
    pub fn new(draft: EventDraft, origin: EventOrigin, occurred_at: DateTime<Utc>) -> Self {
        Self::from_parts(EventEnvelopeParts {
            event_id: EventId::generate(),
            event_type: draft.event_type,
            event_version: draft.event_version,
            occurred_at,
            producer: origin.producer,
            aggregate: draft.aggregate,
            correlation_id: origin.correlation_id,
            causation_id: origin.causation_id,
            principal: origin.principal,
            idempotency_key: origin.idempotency_key,
            trace_context: origin.trace_context,
            payload: draft.payload,
        })
    }

    pub fn from_parts(parts: EventEnvelopeParts) -> Self {
        let EventEnvelopeParts {
            event_id,
            event_type,
            event_version,
            occurred_at,
            producer,
            aggregate,
            correlation_id,
            causation_id,
            principal,
            idempotency_key,
            trace_context,
            payload,
        } = parts;
        Self {
            event_id,
            event_type,
            event_version,
            occurred_at,
            producer,
            aggregate,
            correlation_id,
            causation_id,
            principal,
            idempotency_key,
            trace_context,
            payload,
        }
    }

    pub fn into_parts(self) -> EventEnvelopeParts {
        EventEnvelopeParts {
            event_id: self.event_id,
            event_type: self.event_type,
            event_version: self.event_version,
            occurred_at: self.occurred_at,
            producer: self.producer,
            aggregate: self.aggregate,
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            principal: self.principal,
            idempotency_key: self.idempotency_key,
            trace_context: self.trace_context,
            payload: self.payload,
        }
    }

    pub fn event_id(&self) -> EventId {
        self.event_id
    }

    pub fn event_type(&self) -> &EventType {
        &self.event_type
    }

    pub fn event_version(&self) -> EventVersion {
        self.event_version
    }

    /// The CloudEvents `type` attribute.
    pub fn qualified_type(&self) -> String {
        qualified_event_type(&self.event_type, self.event_version)
    }

    /// The CloudEvents `source` attribute.
    pub fn source(&self) -> String {
        event_source(&self.producer)
    }

    pub fn occurred_at(&self) -> DateTime<Utc> {
        self.occurred_at
    }

    pub fn producer(&self) -> &ServiceName {
        &self.producer
    }

    pub fn aggregate(&self) -> &AggregateRef {
        &self.aggregate
    }

    pub fn correlation_id(&self) -> &RequestId {
        &self.correlation_id
    }

    pub fn causation_id(&self) -> &RequestId {
        &self.causation_id
    }

    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.trace_context.as_ref()
    }

    pub fn payload(&self) -> &EventPayload {
        &self.payload
    }

    fn event_id_as_request_id(&self) -> RequestId {
        RequestId::new(self.event_id.to_string())
            .expect("a hyphenated UUID is a valid message identifier")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggregateId, AggregateType};
    use panel_context::Actor;

    fn draft(event_type: &str) -> EventDraft {
        EventDraft::new(
            EventType::new(event_type).unwrap(),
            EventVersion::V1,
            AggregateRef::new(
                AggregateType::new("revision").unwrap(),
                AggregateId::new("42").unwrap(),
            ),
            EventPayload::json(&serde_json::json!({"revision": 42})).unwrap(),
        )
    }

    fn request_origin() -> EventOrigin {
        EventOrigin::request(
            ServiceName::new("config-service").unwrap(),
            &RequestId::new("req-1").unwrap(),
            &RequestId::new("corr-1").unwrap(),
            Principal::user(Actor::new("user-7").unwrap()),
        )
        .with_idempotency_key(IdempotencyKey::new("apply-42").unwrap())
        .with_trace_context(
            TraceContext::parse(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn request_events_are_caused_by_the_request() {
        let event = EventEnvelope::new(
            draft("config.revision.applied"),
            request_origin(),
            Utc::now(),
        );
        assert_eq!(event.correlation_id().as_str(), "corr-1");
        assert_eq!(event.causation_id().as_str(), "req-1");
        assert_eq!(event.idempotency_key().unwrap().as_str(), "apply-42");
        assert_eq!(
            event.qualified_type(),
            "io.github.eltavine.pingora-panel.config.revision.applied.v1"
        );
        assert_eq!(event.source(), "/pingora-panel/config-service");
    }

    #[test]
    fn scoped_events_carry_the_request_identity_and_trace() {
        let scope = RequestScope::new(RequestId::new("req-7").unwrap())
            .with_correlation_id(RequestId::new("corr-7").unwrap())
            .with_trace_context(TraceContext::parse(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                Some("rojo=1"),
            ));
        let event = EventEnvelope::new(
            draft("config.revision.applied"),
            EventOrigin::scoped(
                ServiceName::new("config-service").unwrap(),
                &scope,
                Principal::user(Actor::new("user-7").unwrap()),
            ),
            Utc::now(),
        );
        assert_eq!(event.causation_id().as_str(), "req-7");
        assert_eq!(event.correlation_id().as_str(), "corr-7");
        assert_eq!(event.trace_context(), scope.trace_context());
    }

    #[test]
    fn follow_up_events_inherit_correlation_and_point_at_their_cause() {
        let parent = EventEnvelope::new(
            draft("config.revision.applied"),
            request_origin(),
            Utc::now(),
        );
        let child = EventEnvelope::new(
            draft("audit.record.appended"),
            EventOrigin::caused_by(ServiceName::new("audit-writer").unwrap(), &parent),
            Utc::now(),
        );
        let grandchild = EventEnvelope::new(
            draft("observability.alert.raised"),
            EventOrigin::caused_by(ServiceName::new("observability-service").unwrap(), &child),
            Utc::now(),
        );

        assert_eq!(child.correlation_id(), parent.correlation_id());
        assert_eq!(child.causation_id().as_str(), parent.event_id().to_string());
        assert_eq!(grandchild.correlation_id().as_str(), "corr-1");
        assert_eq!(
            grandchild.causation_id().as_str(),
            child.event_id().to_string()
        );
        assert_eq!(grandchild.principal(), parent.principal());
        assert_eq!(grandchild.trace_context(), parent.trace_context());
        assert_ne!(child.event_id(), parent.event_id());
    }

    #[test]
    fn event_ids_are_time_ordered_and_round_trip() {
        let first = EventId::generate();
        let second = EventId::generate();
        assert!(first < second);
        assert_eq!(EventId::parse(&first.to_string()).unwrap(), first);
        assert_eq!(EventId::from_bytes(*first.as_bytes()).unwrap(), first);
        assert!(EventId::parse("not-a-uuid").is_err());
        assert!(EventId::parse("00000000-0000-0000-0000-000000000000").is_err());
        let json = serde_json::to_string(&first).unwrap();
        assert_eq!(serde_json::from_str::<EventId>(&json).unwrap(), first);
    }

    #[test]
    fn qualified_types_and_sources_round_trip() {
        let event_type = EventType::new("job.progress.updated").unwrap();
        let version = EventVersion::new(12).unwrap();
        let qualified = qualified_event_type(&event_type, version);
        assert_eq!(
            parse_qualified_event_type(&qualified).unwrap(),
            (event_type, version)
        );
        for foreign in [
            "com.example.job.progress.updated.v1",
            "io.github.eltavine.pingora-panel.job.progress.updated",
            "io.github.eltavine.pingora-panel.job.progress.updated.v0",
            "io.github.eltavine.pingora-panel.job.progress.updated.v01",
            "io.github.eltavine.pingora-panel.job.progress.updated.vx",
            "io.github.eltavine.pingora-panelx.job.updated.v1",
        ] {
            assert!(parse_qualified_event_type(foreign).is_err(), "{foreign}");
        }
        let producer = ServiceName::new("automation-service").unwrap();
        assert_eq!(
            parse_event_source(&event_source(&producer)).unwrap(),
            producer
        );
        assert!(parse_event_source("https://example.com/automation-service").is_err());
        assert!(EventVersion::new(0).is_err());
    }

    #[test]
    fn parts_round_trip_without_loss() {
        let event = EventEnvelope::new(
            draft("config.revision.applied"),
            request_origin(),
            Utc::now(),
        );
        assert_eq!(EventEnvelope::from_parts(event.clone().into_parts()), event);
    }
}
