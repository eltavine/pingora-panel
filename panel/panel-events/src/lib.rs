#![forbid(unsafe_code)]

//! Transport-neutral domain events modelled on CloudEvents 1.0.
//!
//! Every event is attributable to a principal, correlated with the request
//! that started the work, linked to its direct cause and safe to deliver more
//! than once. Brokers, databases and event formats implement the ports
//! declared here in leaf adapters; this crate knows none of them.

mod delivery;
mod envelope;
mod inbox;
mod names;
mod payload;
mod principal;
mod trace;

pub use delivery::{EventDelivery, EventHandler, EventPublisher, HandlerOutcome, PublishReceipt};
pub use envelope::{
    event_source, parse_event_source, parse_qualified_event_type, qualified_event_type, EventDraft,
    EventEnvelope, EventEnvelopeParts, EventId, EventOrigin, EventVersion,
    CLOUDEVENTS_SPEC_VERSION, EVENT_SOURCE_PREFIX, EVENT_TYPE_NAMESPACE,
};
pub use inbox::{
    IdempotentEventHandler, InboxClaim, MemoryProcessedEventStore, ProcessedEventStore,
};
pub use names::{AggregateId, AggregateRef, AggregateType, ConsumerName, EventType, ServiceName};
pub use panel_context::{Actor, IdempotencyKey, RequestId};
pub use payload::{
    EventPayload, JSON_MEDIA_TYPE, MAX_DATA_BYTES, MAX_EVENT_BYTES, PROTOBUF_MEDIA_TYPE,
    PROTOBUF_TYPE_URL_PREFIX,
};
pub use principal::{Principal, PrincipalKind};
pub use trace::{TraceContext, TRACESTATE_PROPAGATION_LIMIT};
