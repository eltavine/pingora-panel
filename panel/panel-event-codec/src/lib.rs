#![forbid(unsafe_code)]

//! CloudEvents 1.0 representations of domain events.
//!
//! * [`protobuf`]: the CloudEvents Protobuf format, used for durable storage
//!   such as the transactional outbox.
//! * [`json`]: the CloudEvents JSON format, which every CloudEvents
//!   implementation must support and which structured-mode bindings carry.
//! * [`binary`]: binary content mode, mapping context attributes onto
//!   `ce-` prefixed, percent-encoded protocol headers.
//!
//! All three share one attribute mapping, so a stored event and a broker
//! message describe the same occurrence identically. Generated types stay
//! inside this crate.

mod attributes;
pub mod binary;
pub mod json;
pub mod protobuf;

/// Media type of a structured-mode CloudEvent in the Protobuf format.
pub const CLOUDEVENTS_PROTOBUF_MEDIA_TYPE: &str = "application/cloudevents+protobuf";
/// Media type of a structured-mode CloudEvent in the JSON format.
pub const CLOUDEVENTS_JSON_MEDIA_TYPE: &str = "application/cloudevents+json";

#[cfg(test)]
mod tests;
