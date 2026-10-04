#![forbid(unsafe_code)]

use async_trait::async_trait;
use chrono::Utc;
use panel_errors::{PanelError, Result};
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, ConsumerName, EventDraft, EventEnvelope,
    EventId, EventOrigin, EventPayload, EventPublisher, EventType, EventVersion, InboxClaim,
    Principal, ProcessedEventStore, PublishReceipt, RequestId, ServiceName,
};
use panel_outbox::{OutboxRelay, OutboxSource, OutboxWakeup, RelayOptions};
use panel_sqlite::{
    testing::TestDatabase, SchemaMigration, SqliteOutbox, SqliteProcessedEventStore,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const REVISIONS: &[SchemaMigration] = &[SchemaMigration::new(
    SchemaMigration::SERVICE_VERSION_FLOOR,
    "revisions",
    "CREATE TABLE revisions (id INTEGER PRIMARY KEY) STRICT;",
)];

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
        match failures.compare_exchange_weak(left, left - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(actual) => left = actual,
        }
    }
    false
}

#[derive(Default)]
struct RecordingPublisher {
    failures: AtomicUsize,
    delivered: Mutex<Vec<EventEnvelope>>,
}

#[async_trait]
impl EventPublisher for RecordingPublisher {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<PublishReceipt> {
        if take_failure(&self.failures) {
            return Err(PanelError::storage_unavailable("broker unavailable"));
        }
        self.delivered.lock().unwrap().push(envelope.clone());
        Ok(PublishReceipt::default())
    }
}

#[tokio::test]
async fn events_exist_exactly_when_their_transaction_commits() {
    let test = TestDatabase::migrated(REVISIONS).await;
    let database = test.database();
    let outbox = SqliteOutbox::new(database);

    let mut rolled_back = database.begin().await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)")
        .execute(&mut *rolled_back)
        .await
        .unwrap();
    SqliteOutbox::append(&mut rolled_back, &event("1", 1))
        .await
        .unwrap();
    rolled_back.rollback().await.unwrap();
    assert!(outbox.pending(10).await.unwrap().is_empty());

    let committed_event = event("1", 2);
    let mut committed = database.begin().await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)")
        .execute(&mut *committed)
        .await
        .unwrap();
    SqliteOutbox::append(&mut committed, &committed_event)
        .await
        .unwrap();
    assert!(outbox.pending(10).await.unwrap().is_empty());
    committed.commit().await.unwrap();

    let pending = outbox.pending(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].envelope(), &committed_event);
    let backlog = outbox.backlog().await.unwrap();
    assert_eq!(backlog.pending, 1);
    assert!(backlog.oldest_recorded_at.is_some());

    let mut connection = database.pool().acquire().await.unwrap();
    let duplicate = SqliteOutbox::append(&mut connection, &committed_event)
        .await
        .unwrap_err();
    assert_eq!(duplicate.code.as_str(), "CONFLICT");
}

#[tokio::test]
async fn a_reported_commit_wakes_the_relay_and_otherwise_it_polls() {
    let test = TestDatabase::migrated(&[]).await;
    let outbox = SqliteOutbox::new(test.database());
    let wakeup = outbox.wakeup();

    let woken = tokio::spawn(async move {
        let started = Instant::now();
        wakeup.wait(Duration::from_secs(5)).await;
        (started.elapsed(), wakeup)
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    test.database().committed();
    let (elapsed, wakeup) = woken.await.unwrap();
    assert!(elapsed < Duration::from_millis(200), "{elapsed:?}");

    let started = Instant::now();
    wakeup.wait(Duration::from_secs(5)).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn the_relay_publishes_in_append_order_and_retries_failures_first() {
    let test = TestDatabase::migrated(&[]).await;
    let outbox = SqliteOutbox::new(test.database());
    let mut connection = test.database().pool().acquire().await.unwrap();
    let events = (1..=4)
        .map(|sequence| event(if sequence % 2 == 0 { "a" } else { "b" }, sequence))
        .collect::<Vec<_>>();
    for event in &events {
        SqliteOutbox::append(&mut connection, event).await.unwrap();
    }
    drop(connection);

    let publisher = Arc::new(RecordingPublisher {
        failures: AtomicUsize::new(1),
        ..RecordingPublisher::default()
    });
    let relay = OutboxRelay::new(
        Arc::new(outbox.clone()),
        Arc::clone(&publisher) as Arc<dyn EventPublisher>,
        Arc::new(outbox.wakeup()),
        RelayOptions::default().with_batch_size(3).unwrap(),
    );

    let failed = relay.relay_once().await.unwrap();
    assert_eq!(failed.published, 0);
    let pending = outbox.pending(10).await.unwrap();
    assert_eq!(pending.len(), 4);
    assert_eq!(pending[0].attempts(), 1);

    assert_eq!(relay.relay_once().await.unwrap().published, 3);
    assert_eq!(relay.relay_once().await.unwrap().published, 1);
    assert_eq!(relay.relay_once().await.unwrap().published, 0);
    assert_eq!(*publisher.delivered.lock().unwrap(), events);
    assert_eq!(outbox.backlog().await.unwrap().pending, 0);

    assert_eq!(
        outbox
            .purge_published(Duration::from_secs(3600), 100)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        outbox.purge_published(Duration::ZERO, 100).await.unwrap(),
        4
    );
}

#[tokio::test]
async fn consumers_claim_complete_and_release_events_once() {
    let test = TestDatabase::migrated(&[]).await;
    let inbox = SqliteProcessedEventStore::new(test.database());
    let consumer = ConsumerName::new("audit").unwrap();
    let first = EventId::generate();
    let lease = Duration::from_secs(60);

    assert_eq!(
        inbox.claim(&consumer, first, lease).await.unwrap(),
        InboxClaim::Acquired
    );
    assert_eq!(
        inbox.claim(&consumer, first, lease).await.unwrap(),
        InboxClaim::InFlight
    );
    inbox.release(&consumer, first).await.unwrap();
    assert_eq!(
        inbox.claim(&consumer, first, Duration::ZERO).await.unwrap(),
        InboxClaim::Acquired
    );
    assert_eq!(
        inbox.claim(&consumer, first, lease).await.unwrap(),
        InboxClaim::Acquired,
        "a lapsed claim is taken over"
    );
    inbox.complete(&consumer, first).await.unwrap();
    assert_eq!(
        inbox.claim(&consumer, first, lease).await.unwrap(),
        InboxClaim::AlreadyProcessed
    );

    let second = EventId::generate();
    let mut transaction = test.database().begin().await.unwrap();
    assert!(
        SqliteProcessedEventStore::record_in(&mut transaction, &consumer, second)
            .await
            .unwrap()
    );
    assert!(
        !SqliteProcessedEventStore::record_in(&mut transaction, &consumer, second)
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();

    assert_eq!(
        inbox
            .purge_processed(Duration::from_secs(3600), 100)
            .await
            .unwrap(),
        0
    );
    assert_eq!(inbox.purge_processed(Duration::ZERO, 100).await.unwrap(), 2);
}
