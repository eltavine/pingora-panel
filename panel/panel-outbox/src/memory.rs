use crate::{OutboxPosition, OutboxRecord, OutboxSource, OutboxWakeup};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use panel_events::EventEnvelope;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct State {
    next: u64,
    pending: BTreeMap<OutboxPosition, (EventEnvelope, u32)>,
    published: Vec<EventEnvelope>,
}

/// Process-local outbox for tests and single-process composition.
#[derive(Clone, Default)]
pub struct MemoryOutbox {
    state: Arc<Mutex<State>>,
    appended: Arc<Notify>,
}

impl MemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, envelope: EventEnvelope) -> Result<OutboxPosition> {
        let mut state = self.lock()?;
        state.next += 1;
        let position = OutboxPosition::new(state.next);
        state.pending.insert(position, (envelope, 0));
        drop(state);
        self.appended.notify_one();
        Ok(position)
    }

    pub fn pending_count(&self) -> usize {
        self.lock()
            .map(|state| state.pending.len())
            .unwrap_or_default()
    }

    /// Events marked published, in publication order.
    pub fn published(&self) -> Vec<EventEnvelope> {
        self.lock()
            .map(|state| state.published.clone())
            .unwrap_or_default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| PanelError::internal("in-memory outbox lock is poisoned"))
    }
}

#[async_trait]
impl OutboxSource for MemoryOutbox {
    async fn pending(&self, limit: usize) -> Result<Vec<OutboxRecord>> {
        Ok(self
            .lock()?
            .pending
            .iter()
            .take(limit)
            .map(|(position, (envelope, attempts))| {
                OutboxRecord::new(*position, envelope.clone(), *attempts)
            })
            .collect())
    }

    async fn mark_published(&self, positions: &[OutboxPosition]) -> Result<()> {
        let mut state = self.lock()?;
        for position in positions {
            if let Some((envelope, _)) = state.pending.remove(position) {
                state.published.push(envelope);
            }
        }
        Ok(())
    }

    async fn record_failure(&self, position: OutboxPosition, _reason: &str) -> Result<()> {
        if let Some((_, attempts)) = self.lock()?.pending.get_mut(&position) {
            *attempts += 1;
        }
        Ok(())
    }
}

#[async_trait]
impl OutboxWakeup for MemoryOutbox {
    async fn wait(&self, timeout: Duration) {
        let _ = tokio::time::timeout(timeout, self.appended.notified()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutboxRelay, RelayOptions};
    use chrono::Utc;
    use panel_events::{
        Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventOrigin, EventPayload,
        EventPublisher, EventType, EventVersion, Principal, PublishReceipt, RequestId, ServiceName,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn event(aggregate: &str, sequence: u32) -> EventEnvelope {
        EventEnvelope::new(
            EventDraft::new(
                EventType::new("config.revision.changed").unwrap(),
                EventVersion::V1,
                AggregateRef::new(
                    AggregateType::new("revision").unwrap(),
                    AggregateId::new(aggregate).unwrap(),
                ),
                EventPayload::json(&serde_json::json!({ "sequence": sequence })).unwrap(),
            ),
            EventOrigin::request(
                ServiceName::new("config-service").unwrap(),
                &RequestId::new("req").unwrap(),
                &RequestId::new("corr").unwrap(),
                Principal::system(Actor::new("config-service").unwrap()),
            ),
            Utc::now(),
        )
    }

    /// Takes one of the failures left, if any.
    fn take_failure(failures: &AtomicUsize) -> bool {
        let mut left = failures.load(Ordering::SeqCst);
        while left > 0 {
            match failures.compare_exchange_weak(left, left - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(actual) => left = actual,
            }
        }
        false
    }

    /// Fails publication for the first `failures` attempts.
    #[derive(Default)]
    struct FlakyPublisher {
        failures: AtomicUsize,
        delivered: Mutex<Vec<EventEnvelope>>,
    }

    #[async_trait]
    impl EventPublisher for FlakyPublisher {
        async fn publish(&self, envelope: &EventEnvelope) -> Result<PublishReceipt> {
            if take_failure(&self.failures) {
                return Err(PanelError::storage_unavailable("broker unavailable"));
            }
            self.delivered.lock().unwrap().push(envelope.clone());
            Ok(PublishReceipt::default())
        }
    }

    fn relay(outbox: &MemoryOutbox, publisher: Arc<FlakyPublisher>) -> OutboxRelay {
        OutboxRelay::new(
            Arc::new(outbox.clone()),
            publisher,
            Arc::new(outbox.clone()),
            RelayOptions::default()
                .with_batch_size(10)
                .unwrap()
                .with_backoff(Duration::from_millis(1), Duration::from_millis(5))
                .unwrap()
                .with_idle_poll(Duration::from_millis(20)),
        )
    }

    #[tokio::test]
    async fn a_failure_stops_the_batch_so_later_events_never_overtake() {
        let outbox = MemoryOutbox::new();
        let events = (1..=3)
            .map(|sequence| event("7", sequence))
            .collect::<Vec<_>>();
        for event in &events {
            outbox.append(event.clone()).unwrap();
        }
        let publisher = Arc::new(FlakyPublisher {
            failures: AtomicUsize::new(1),
            ..FlakyPublisher::default()
        });
        let relay = relay(&outbox, Arc::clone(&publisher));

        let first = relay.relay_once().await.unwrap();
        assert_eq!(first.published, 0);
        assert_eq!(first.failed, Some(OutboxPosition::new(1)));
        assert_eq!(outbox.pending_count(), 3);
        assert_eq!(outbox.pending(1).await.unwrap()[0].attempts(), 1);

        let second = relay.relay_once().await.unwrap();
        assert_eq!(second.published, 3);
        assert_eq!(outbox.published(), events);
        assert_eq!(*publisher.delivered.lock().unwrap(), events);
    }

    #[tokio::test]
    async fn the_running_relay_publishes_new_commits_promptly_and_stops_on_shutdown() {
        let outbox = MemoryOutbox::new();
        let publisher = Arc::new(FlakyPublisher {
            failures: AtomicUsize::new(2),
            ..FlakyPublisher::default()
        });
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(relay(&outbox, Arc::clone(&publisher)).run(async move {
            let _ = stopped.await;
        }));

        let appended = (1..=5)
            .map(|sequence| {
                let event = event("9", sequence);
                outbox.append(event.clone()).unwrap();
                event
            })
            .collect::<Vec<_>>();
        tokio::time::timeout(Duration::from_secs(5), async {
            while outbox.pending_count() > 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("relay drains the outbox");

        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("relay stops on shutdown")
            .unwrap();
        assert_eq!(outbox.published(), appended);
    }

    #[test]
    fn options_are_validated() {
        assert!(RelayOptions::default().with_batch_size(0).is_err());
        assert!(RelayOptions::default()
            .with_backoff(Duration::from_secs(2), Duration::from_secs(1))
            .is_err());
    }
}
