#![forbid(unsafe_code)]

use panel_events::{ConsumerName, EventId, InboxClaim, ProcessedEventStore};
use panel_postgres::{testing::TestDatabase, PgProcessedEventStore, ServiceDatabase};
use std::time::Duration;

async fn migrated(database: &mut TestDatabase) -> ServiceDatabase {
    let secrets = database.bootstrap(&[("automation", "automation")]).await;
    let service = database
        .connect_service("automation", "automation", &secrets[0])
        .await;
    service.migrate(&[]).await.unwrap();
    service
}

#[tokio::test]
async fn claims_are_exclusive_until_completed_released_or_expired() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    let store = PgProcessedEventStore::new(&service);
    let consumer = ConsumerName::new("certificate-renewal").unwrap();
    let other = ConsumerName::new("audit-projection").unwrap();
    let event = EventId::generate();
    let lease = Duration::from_secs(30);

    assert_eq!(
        store.claim(&consumer, event, lease).await.unwrap(),
        InboxClaim::Acquired
    );
    assert_eq!(
        store.claim(&consumer, event, lease).await.unwrap(),
        InboxClaim::InFlight
    );
    assert_eq!(
        store.claim(&other, event, lease).await.unwrap(),
        InboxClaim::Acquired
    );

    store.release(&consumer, event).await.unwrap();
    assert_eq!(
        store.claim(&consumer, event, lease).await.unwrap(),
        InboxClaim::Acquired
    );
    store.complete(&consumer, event).await.unwrap();
    assert_eq!(
        store.claim(&consumer, event, lease).await.unwrap(),
        InboxClaim::AlreadyProcessed
    );
    store.release(&consumer, event).await.unwrap();
    assert_eq!(
        store.claim(&consumer, event, lease).await.unwrap(),
        InboxClaim::AlreadyProcessed,
        "releasing must never undo a completion"
    );

    let expiring = EventId::generate();
    assert_eq!(
        store
            .claim(&consumer, expiring, Duration::ZERO)
            .await
            .unwrap(),
        InboxClaim::Acquired
    );
    assert_eq!(
        store.claim(&consumer, expiring, lease).await.unwrap(),
        InboxClaim::Acquired
    );

    assert_eq!(store.purge_processed(Duration::ZERO, 100).await.unwrap(), 1);
    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn transactional_records_commit_with_the_handler_effects() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let service = migrated(&mut database).await;
    let consumer = ConsumerName::new("site-projection").unwrap();
    let event = EventId::generate();
    sqlx::query("CREATE TABLE projected (event_id TEXT PRIMARY KEY)")
        .execute(service.pool())
        .await
        .unwrap();

    let mut rolled_back = service.pool().begin().await.unwrap();
    assert!(
        PgProcessedEventStore::record_in(&mut rolled_back, &consumer, event)
            .await
            .unwrap()
    );
    rolled_back.rollback().await.unwrap();

    for expected_first in [true, false] {
        let mut transaction = service.pool().begin().await.unwrap();
        let first = PgProcessedEventStore::record_in(&mut transaction, &consumer, event)
            .await
            .unwrap();
        assert_eq!(first, expected_first);
        if first {
            sqlx::query("INSERT INTO projected VALUES ($1)")
                .bind(event.to_string())
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        transaction.commit().await.unwrap();
    }
    let projected: i64 = sqlx::query_scalar("SELECT count(*) FROM projected")
        .fetch_one(service.pool())
        .await
        .unwrap();
    assert_eq!(projected, 1);

    service.close().await;
    database.drop().await;
}
