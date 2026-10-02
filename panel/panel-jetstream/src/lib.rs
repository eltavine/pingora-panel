#![forbid(unsafe_code)]

//! NATS JetStream adapter for domain events.
//!
//! Events travel as CloudEvents in the NATS protocol binding's binary content
//! mode; structured JSON messages are accepted on receipt. Streams are
//! provisioned idempotently, publications are deduplicated by event ID,
//! consumers acknowledge explicitly, and events that cannot be processed are
//! parked in a dead-letter queue from which an operator can replay them to
//! the consumer that failed.

mod consumer;
mod dead_letter;
mod error;
mod message;
mod publisher;
mod settings;
mod topology;

pub use consumer::{ConsumerSpec, JetStreamConsumer};
pub use dead_letter::{DeadLetterQueue, DeadLetterRecord, ReplayReceipt};
pub use publisher::JetStreamPublisher;
pub use settings::JetStreamSettings;
pub use topology::{ensure_streams, StreamChange};

#[cfg(feature = "test-support")]
pub mod testing;
