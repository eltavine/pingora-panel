use crate::{ConsumerName, EventDelivery, EventHandler, EventId, HandlerOutcome};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Result of claiming one event for one consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InboxClaim {
    /// The caller owns processing until it completes, releases or the lease lapses.
    Acquired,
    /// The consumer already finished this event.
    AlreadyProcessed,
    /// Another delivery holds an unexpired claim.
    InFlight,
}

/// Durable record of the events each consumer has finished.
///
/// `claim` must be linearizable per `(consumer, event)` so two concurrent
/// deliveries cannot both acquire it. A failed `complete` must leave the
/// claim held; the lease bounds how long a crashed worker blocks retries.
#[async_trait]
pub trait ProcessedEventStore: Send + Sync {
    async fn claim(
        &self,
        consumer: &ConsumerName,
        event_id: EventId,
        lease: Duration,
    ) -> Result<InboxClaim>;

    async fn complete(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()>;

    async fn release(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()>;
}

/// Makes an at-least-once handler effectively-once per consumer.
pub struct IdempotentEventHandler<H> {
    inner: H,
    store: Arc<dyn ProcessedEventStore>,
    lease: Duration,
    unavailable_retry: Duration,
}

impl<H> IdempotentEventHandler<H> {
    pub fn new(inner: H, store: Arc<dyn ProcessedEventStore>, lease: Duration) -> Self {
        Self {
            inner,
            store,
            lease,
            unavailable_retry: Duration::from_secs(1),
        }
    }

    /// Delay before redelivery when the inbox itself cannot be reached.
    pub fn with_unavailable_retry(mut self, delay: Duration) -> Self {
        self.unavailable_retry = delay;
        self
    }
}

#[async_trait]
impl<H: EventHandler> EventHandler for IdempotentEventHandler<H> {
    async fn handle(&self, delivery: &EventDelivery) -> HandlerOutcome {
        let consumer = delivery.consumer();
        let event_id = delivery.envelope().event_id();
        match self.store.claim(consumer, event_id, self.lease).await {
            Ok(InboxClaim::Acquired) => {}
            Ok(InboxClaim::AlreadyProcessed) => return HandlerOutcome::Ack,
            Ok(InboxClaim::InFlight) => {
                return HandlerOutcome::retry(self.lease, "event is being processed elsewhere")
            }
            Err(error) => return HandlerOutcome::retry(self.unavailable_retry, error.to_string()),
        }
        let outcome = self.inner.handle(delivery).await;
        match outcome {
            HandlerOutcome::Ack => match self.store.complete(consumer, event_id).await {
                Ok(()) => HandlerOutcome::Ack,
                // The claim stays held, so a redelivery waits for the lease
                // instead of racing a second execution.
                Err(error) => HandlerOutcome::retry(self.lease, error.to_string()),
            },
            other => {
                if let Err(error) = self.store.release(consumer, event_id).await {
                    return HandlerOutcome::retry(self.lease, error.to_string());
                }
                other
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum MemoryInboxState {
    Claimed { until: Instant },
    Processed,
}

/// Process-local inbox for tests and single-process composition.
///
/// It does not survive a restart; durable deployments use a database adapter.
#[derive(Clone, Default)]
pub struct MemoryProcessedEventStore {
    entries: Arc<Mutex<HashMap<(ConsumerName, EventId), MemoryInboxState>>>,
}

impl MemoryProcessedEventStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_processed(&self, consumer: &ConsumerName, event_id: EventId) -> bool {
        matches!(
            self.lock()
                .ok()
                .and_then(|entries| entries.get(&(consumer.clone(), event_id)).copied()),
            Some(MemoryInboxState::Processed)
        )
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<(ConsumerName, EventId), MemoryInboxState>>> {
        self.entries
            .lock()
            .map_err(|_| PanelError::internal("in-memory inbox lock is poisoned"))
    }
}

#[async_trait]
impl ProcessedEventStore for MemoryProcessedEventStore {
    async fn claim(
        &self,
        consumer: &ConsumerName,
        event_id: EventId,
        lease: Duration,
    ) -> Result<InboxClaim> {
        let now = Instant::now();
        let mut entries = self.lock()?;
        let key = (consumer.clone(), event_id);
        match entries.get(&key) {
            Some(MemoryInboxState::Processed) => Ok(InboxClaim::AlreadyProcessed),
            Some(MemoryInboxState::Claimed { until }) if *until > now => Ok(InboxClaim::InFlight),
            _ => {
                let until = now.checked_add(lease).unwrap_or(now);
                entries.insert(key, MemoryInboxState::Claimed { until });
                Ok(InboxClaim::Acquired)
            }
        }
    }

    async fn complete(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()> {
        self.lock()?
            .insert((consumer.clone(), event_id), MemoryInboxState::Processed);
        Ok(())
    }

    async fn release(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()> {
        let mut entries = self.lock()?;
        let key = (consumer.clone(), event_id);
        if matches!(entries.get(&key), Some(MemoryInboxState::Claimed { .. })) {
            entries.remove(&key);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
        EventPayload, EventType, EventVersion, Principal, ServiceName,
    };
    use chrono::Utc;
    use panel_context::{Actor, RequestId};
    use std::{
        num::NonZeroU32,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct CountingHandler {
        calls: AtomicUsize,
        outcome: Mutex<HandlerOutcome>,
    }

    impl CountingHandler {
        fn new(outcome: HandlerOutcome) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(outcome),
            })
        }

        fn set(&self, outcome: HandlerOutcome) {
            *self.outcome.lock().unwrap() = outcome;
        }
    }

    #[async_trait]
    impl EventHandler for CountingHandler {
        async fn handle(&self, _delivery: &EventDelivery) -> HandlerOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome.lock().unwrap().clone()
        }
    }

    struct FailingCompletion(MemoryProcessedEventStore);

    #[async_trait]
    impl ProcessedEventStore for FailingCompletion {
        async fn claim(
            &self,
            consumer: &ConsumerName,
            event_id: EventId,
            lease: Duration,
        ) -> Result<InboxClaim> {
            self.0.claim(consumer, event_id, lease).await
        }

        async fn complete(&self, _consumer: &ConsumerName, _event_id: EventId) -> Result<()> {
            Err(PanelError::storage_unavailable("inbox write failed"))
        }

        async fn release(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()> {
            self.0.release(consumer, event_id).await
        }
    }

    fn delivery(attempt: u32) -> EventDelivery {
        let envelope = EventEnvelope::new(
            EventDraft::new(
                EventType::new("config.revision.activated").unwrap(),
                EventVersion::V1,
                AggregateRef::new(
                    AggregateType::new("revision").unwrap(),
                    AggregateId::new("7").unwrap(),
                ),
                EventPayload::json(&7).unwrap(),
            ),
            EventOrigin::request(
                ServiceName::new("config-service").unwrap(),
                &RequestId::new("req").unwrap(),
                &RequestId::new("corr").unwrap(),
                Principal::user(Actor::new("operator").unwrap()),
            ),
            Utc::now(),
        );
        EventDelivery::new(
            envelope,
            ConsumerName::new("audit-projection").unwrap(),
            NonZeroU32::new(attempt).unwrap(),
        )
    }

    #[tokio::test]
    async fn redelivered_events_run_the_handler_once() {
        let store = MemoryProcessedEventStore::new();
        let inner = CountingHandler::new(HandlerOutcome::Ack);
        let handler = IdempotentEventHandler::new(
            Arc::clone(&inner),
            Arc::new(store.clone()),
            Duration::from_secs(30),
        );
        let first = delivery(1);
        let redelivered = EventDelivery::new(
            first.envelope().clone(),
            first.consumer().clone(),
            NonZeroU32::new(2).unwrap(),
        );

        assert_eq!(handler.handle(&first).await, HandlerOutcome::Ack);
        assert_eq!(handler.handle(&redelivered).await, HandlerOutcome::Ack);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert!(store.is_processed(first.consumer(), first.envelope().event_id()));
    }

    #[tokio::test]
    async fn failed_attempts_release_the_claim_for_a_retry() {
        let store = MemoryProcessedEventStore::new();
        let inner = CountingHandler::new(HandlerOutcome::retry(Duration::from_millis(1), "busy"));
        let handler = IdempotentEventHandler::new(
            Arc::clone(&inner),
            Arc::new(store.clone()),
            Duration::from_secs(30),
        );
        let delivery = delivery(1);

        assert!(matches!(
            handler.handle(&delivery).await,
            HandlerOutcome::Retry { .. }
        ));
        inner.set(HandlerOutcome::dead_letter("poison"));
        assert!(matches!(
            handler.handle(&delivery).await,
            HandlerOutcome::DeadLetter { .. }
        ));
        inner.set(HandlerOutcome::Ack);
        assert_eq!(handler.handle(&delivery).await, HandlerOutcome::Ack);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn concurrent_deliveries_wait_for_the_claim_holder() {
        let store = MemoryProcessedEventStore::new();
        let delivery = delivery(1);
        let lease = Duration::from_secs(30);
        assert_eq!(
            store
                .claim(delivery.consumer(), delivery.envelope().event_id(), lease)
                .await
                .unwrap(),
            InboxClaim::Acquired
        );
        let inner = CountingHandler::new(HandlerOutcome::Ack);
        let handler = IdempotentEventHandler::new(Arc::clone(&inner), Arc::new(store), lease);

        assert_eq!(
            handler.handle(&delivery).await,
            HandlerOutcome::retry(lease, "event is being processed elsewhere")
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_claims_can_be_reacquired() {
        let store = MemoryProcessedEventStore::new();
        let delivery = delivery(1);
        let consumer = delivery.consumer();
        let event_id = delivery.envelope().event_id();
        assert_eq!(
            store
                .claim(consumer, event_id, Duration::ZERO)
                .await
                .unwrap(),
            InboxClaim::Acquired
        );
        assert_eq!(
            store
                .claim(consumer, event_id, Duration::from_secs(1))
                .await
                .unwrap(),
            InboxClaim::Acquired
        );
    }

    #[tokio::test]
    async fn completion_failures_keep_the_claim_held() {
        let memory = MemoryProcessedEventStore::new();
        let inner = CountingHandler::new(HandlerOutcome::Ack);
        let lease = Duration::from_secs(30);
        let handler = IdempotentEventHandler::new(
            Arc::clone(&inner),
            Arc::new(FailingCompletion(memory.clone())),
            lease,
        );
        let delivery = delivery(1);

        assert!(
            matches!(handler.handle(&delivery).await, HandlerOutcome::Retry { after, .. } if after == lease)
        );
        assert_eq!(
            memory
                .claim(delivery.consumer(), delivery.envelope().event_id(), lease)
                .await
                .unwrap(),
            InboxClaim::InFlight
        );
    }
}
