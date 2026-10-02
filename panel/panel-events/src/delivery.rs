use crate::{ConsumerName, EventEnvelope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{num::NonZeroU32, time::Duration};

/// Broker acknowledgement for one published event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PublishReceipt {
    sequence: Option<u64>,
    duplicate: bool,
}

impl PublishReceipt {
    pub const fn new(sequence: Option<u64>, duplicate: bool) -> Self {
        Self {
            sequence,
            duplicate,
        }
    }

    /// Broker-assigned position, when the broker exposes one.
    pub const fn sequence(self) -> Option<u64> {
        self.sequence
    }

    /// The broker recognised the event ID and kept the earlier copy.
    pub const fn duplicate(self) -> bool {
        self.duplicate
    }
}

/// Durable, at-least-once event publication.
///
/// `Ok` means the broker durably accepted the event. Publishing the same
/// event ID again must be harmless.
#[async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<PublishReceipt>;
}

/// One delivery attempt of an event to a named consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventDelivery {
    envelope: EventEnvelope,
    consumer: ConsumerName,
    attempt: NonZeroU32,
}

impl EventDelivery {
    pub fn new(envelope: EventEnvelope, consumer: ConsumerName, attempt: NonZeroU32) -> Self {
        Self {
            envelope,
            consumer,
            attempt,
        }
    }

    pub fn envelope(&self) -> &EventEnvelope {
        &self.envelope
    }

    pub fn consumer(&self) -> &ConsumerName {
        &self.consumer
    }

    /// 1-based delivery attempt reported by the broker.
    pub fn attempt(&self) -> NonZeroU32 {
        self.attempt
    }

    pub fn into_envelope(self) -> EventEnvelope {
        self.envelope
    }
}

/// The consumer's decision about one delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HandlerOutcome {
    /// Processing finished; the event must not be delivered again.
    Ack,
    /// Processing failed transiently; redeliver after the delay.
    Retry { after: Duration, reason: String },
    /// The event can never succeed as-is; park it for operator review.
    DeadLetter { reason: String },
}

impl HandlerOutcome {
    pub fn retry(after: Duration, reason: impl Into<String>) -> Self {
        Self::Retry {
            after,
            reason: reason.into(),
        }
    }

    pub fn dead_letter(reason: impl Into<String>) -> Self {
        Self::DeadLetter {
            reason: reason.into(),
        }
    }

    /// Maps a stable error onto a delivery decision: retryable errors are
    /// redelivered, every other error is parked.
    pub fn from_result(result: Result<()>, retry_after: Duration) -> Self {
        match result {
            Ok(()) => Self::Ack,
            Err(error) => Self::from_error(&error, retry_after),
        }
    }

    pub fn from_error(error: &PanelError, retry_after: Duration) -> Self {
        let reason = format!("{}: {}", error.code, error.message);
        if error.retryable {
            Self::retry(retry_after, reason)
        } else {
            Self::dead_letter(reason)
        }
    }
}

/// Event consumer logic, independent of the broker that drives it.
///
/// Delivery is at-least-once, so implementations must be idempotent or be
/// wrapped in [`crate::IdempotentEventHandler`].
#[async_trait]
pub trait EventHandler: Send + Sync {
    async fn handle(&self, delivery: &EventDelivery) -> HandlerOutcome;
}

#[async_trait]
impl<T: EventHandler + ?Sized> EventHandler for std::sync::Arc<T> {
    async fn handle(&self, delivery: &EventDelivery) -> HandlerOutcome {
        (**self).handle(delivery).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_errors_map_onto_delivery_decisions() {
        let delay = Duration::from_secs(5);
        assert_eq!(
            HandlerOutcome::from_result(Ok(()), delay),
            HandlerOutcome::Ack
        );
        assert!(matches!(
            HandlerOutcome::from_error(&PanelError::storage_unavailable("db down"), delay),
            HandlerOutcome::Retry { after, .. } if after == delay
        ));
        assert!(matches!(
            HandlerOutcome::from_error(&PanelError::invalid_argument("bad payload"), delay),
            HandlerOutcome::DeadLetter { reason } if reason.contains("INVALID_ARGUMENT")
        ));
    }

    #[test]
    fn receipts_expose_broker_facts() {
        let receipt = PublishReceipt::new(Some(7), true);
        assert_eq!(receipt.sequence(), Some(7));
        assert!(receipt.duplicate());
        assert_eq!(PublishReceipt::default().sequence(), None);
    }
}
