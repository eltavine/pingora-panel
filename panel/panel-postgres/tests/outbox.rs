#![forbid(unsafe_code)]

use async_trait::async_trait;
use chrono::Utc;
use panel_errors::{PanelError, Result};
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventPublisher, EventType, EventVersion, Principal, PublishReceipt, RequestId,
    ServiceName,
};
use panel_outbox::{OutboxRelay, OutboxSource, OutboxWakeup, RelayOptions};
use panel_postgres::{testing::TestDatabase, PgOutbox, ServiceDatabase};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

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

async fn migrated(database: &mut TestDatabase) -> ServiceDatabase {
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let service = database
        .connect_service("config", "config", &secrets[0])
        .await;
    service.migrate(&[]).await.unwrap();
    service
}

#[derive(Default)]
struct RecordingPublisher {
    failures: AtomicUsize,
    delivered: Mutex<Vec<EventEnvelope>>,
}

#[async_trait]
impl EventPublisher for RecordingPublisher {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<PublishReceipt> {
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(PanelError::storage_unavailable("broker unavailable"));
        }
        self.delivered.lock().unwrap().push(envelope.clone());
        Ok(PublishReceipt::default())
    }
}

#[tokio::test]
async fn events_exist_exactly_when_their_transaction_commits() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    sqlx::query("CREATE TABLE revisions (id BIGINT PRIMARY KEY)")
        .execute(service.pool())
        .await
        .unwrap();
    let outbox = PgOutbox::new(&service);
    let wakeup = outbox.listen().await.unwrap();

    let mut rolled_back = service.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)")
        .execute(&mut *rolled_back)
        .await
        .unwrap();
    PgOutbox::append(&mut rolled_back, &event("1", 1))
        .await
        .unwrap();
    rolled_back.rollback().await.unwrap();
    assert!(outbox.pending(10).await.unwrap().is_empty());

    let committed_event = event("1", 2);
    let mut committed = service.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)")
        .execute(&mut *committed)
        .await
        .unwrap();
    PgOutbox::append(&mut committed, &committed_event)
        .await
        .unwrap();
    assert!(outbox.pending(10).await.unwrap().is_empty());
    committed.commit().await.unwrap();

    let started = std::time::Instant::now();
    wakeup.wait(Duration::from_secs(5)).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "commit must notify"
    );
    let pending = outbox.pending(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].envelope(), &committed_event);
    assert_eq!(outbox.backlog().await.unwrap().pending, 1);

    let mut connection = service.pool().acquire().await.unwrap();
    let duplicate = PgOutbox::append(&mut connection, &committed_event)
        .await
        .unwrap_err();
    drop(connection);
    assert_eq!(duplicate.code.as_str(), "CONFLICT");

    // Closing the pool waits for the listener's connection.
    drop(wakeup);
    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn the_relay_publishes_in_append_order_and_retries_failures_first() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    let outbox = PgOutbox::new(&service);
    let mut connection = service.pool().acquire().await.unwrap();
    let events = (1..=4)
        .map(|sequence| event(if sequence % 2 == 0 { "a" } else { "b" }, sequence))
        .collect::<Vec<_>>();
    for event in &events {
        PgOutbox::append(&mut connection, event).await.unwrap();
    }
    drop(connection);

    let publisher = Arc::new(RecordingPublisher {
        failures: AtomicUsize::new(1),
        ..RecordingPublisher::default()
    });
    let relay = OutboxRelay::new(
        Arc::new(outbox.clone()),
        Arc::clone(&publisher) as Arc<dyn EventPublisher>,
        Arc::new(outbox.listen().await.unwrap()),
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

    // Closing the pool waits for the relay's listener connection.
    drop(relay);
    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn only_one_relay_leads_an_outbox_at_a_time() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    let outbox = PgOutbox::new(&service);

    let leader = outbox.try_lead().await.unwrap().expect("first relay leads");
    assert!(outbox.try_lead().await.unwrap().is_none());
    leader.release().await.unwrap();
    let successor = outbox.try_lead().await.unwrap();
    assert!(successor.is_some());
    successor.unwrap().release().await.unwrap();

    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn a_terminated_leadership_session_is_noticed_and_succeeded() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    let outbox = PgOutbox::new(&service);
    let mut leader = outbox.try_lead().await.unwrap().expect("first relay leads");

    let mut admin = database.admin_connection().await;
    let terminated: Vec<bool> = sqlx::query_scalar(
        "SELECT pg_terminate_backend(pid) FROM pg_locks \
         WHERE locktype = 'advisory' AND granted AND database = \
         (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap();
    assert_eq!(terminated, [true]);

    tokio::time::timeout(
        Duration::from_secs(5),
        leader.lost(Duration::from_millis(20)),
    )
    .await
    .expect("the lost session is noticed");
    drop(leader);
    let successor = outbox.try_lead().await.unwrap();
    assert!(
        successor.is_some(),
        "the lock was released with the session"
    );
    successor.unwrap().release().await.unwrap();

    service.close().await;
    database.drop().await;
}
