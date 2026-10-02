#![forbid(unsafe_code)]

//! Transactional outbox relay.
//!
//! Producers append events to an outbox in the same transaction as their
//! state change, so an event exists exactly when its change committed. One
//! relay per outbox then publishes committed events in append order. It
//! stops at the first failure and retries that event before any later one,
//! which keeps every aggregate's events in order. Delivery is at-least-once:
//! a crash between publishing and recording publication republishes the
//! event, so consumers deduplicate on the event ID.

mod memory;
mod relay;

pub use memory::MemoryOutbox;
pub use relay::{OutboxRelay, RelayOptions, RelayReport};

use async_trait::async_trait;
use panel_errors::Result;
use panel_events::EventEnvelope;
use std::time::Duration;

/// Append-order position of an outbox record.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutboxPosition(u64);

impl OutboxPosition {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A committed event awaiting publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxRecord {
    position: OutboxPosition,
    envelope: EventEnvelope,
    attempts: u32,
}

impl OutboxRecord {
    pub fn new(position: OutboxPosition, envelope: EventEnvelope, attempts: u32) -> Self {
        Self {
            position,
            envelope,
            attempts,
        }
    }

    pub fn position(&self) -> OutboxPosition {
        self.position
    }

    pub fn envelope(&self) -> &EventEnvelope {
        &self.envelope
    }

    /// Failed publication attempts recorded so far.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// Read side of an outbox used by its relay.
#[async_trait]
pub trait OutboxSource: Send + Sync {
    /// Unpublished records, oldest first.
    async fn pending(&self, limit: usize) -> Result<Vec<OutboxRecord>>;

    /// Marks records published. Repeating the call must be harmless.
    async fn mark_published(&self, positions: &[OutboxPosition]) -> Result<()>;

    /// Records a failed publication attempt for diagnostics.
    async fn record_failure(&self, position: OutboxPosition, reason: &str) -> Result<()>;
}

/// Lets an idle relay sleep until new events are committed.
#[async_trait]
pub trait OutboxWakeup: Send + Sync {
    /// Returns after a commit signal or after `timeout`, whichever is first.
    async fn wait(&self, timeout: Duration);
}

/// Wakeup that only sleeps, for outboxes without commit notifications.
#[derive(Clone, Copy, Debug, Default)]
pub struct PollingWakeup;

#[async_trait]
impl OutboxWakeup for PollingWakeup {
    async fn wait(&self, timeout: Duration) {
        tokio::time::sleep(timeout).await;
    }
}
