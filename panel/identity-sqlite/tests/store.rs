#![forbid(unsafe_code)]

use async_trait::async_trait;
use identity_sqlite::{SqliteIdentityStore, MIGRATIONS};
use panel_events::ServiceName;
use panel_identity::{
    conformance::{check, StoreUnderTest},
    memory::RecordedEvent,
    IdentityStore, ProviderStore, RoleStore, WorkloadStore,
};
use panel_sqlite::{testing::TestDatabase, EventLog, ServiceDatabase};
use std::sync::Arc;

struct Sqlite {
    store: Arc<SqliteIdentityStore>,
    database: ServiceDatabase,
}

#[async_trait]
impl StoreUnderTest for Sqlite {
    fn store(&self) -> Arc<dyn IdentityStore> {
        self.store.clone()
    }

    fn providers(&self) -> Arc<dyn ProviderStore> {
        self.store.clone()
    }

    fn workloads(&self) -> Arc<dyn WorkloadStore> {
        self.store.clone()
    }

    async fn events(&self) -> Vec<RecordedEvent> {
        let rows: Vec<Vec<u8>> =
            sqlx::query_scalar("SELECT cloudevent FROM outbox ORDER BY position")
                .fetch_all(self.database.pool())
                .await
                .unwrap();
        rows.iter()
            .map(|bytes| {
                let event = panel_event_codec::protobuf::decode(bytes).unwrap();
                RecordedEvent {
                    event_type: event.event_type().to_string(),
                    actor: event
                        .principal()
                        .id()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    data: serde_json::from_slice(event.payload().data()).unwrap(),
                }
            })
            .collect()
    }
}

#[tokio::test]
async fn the_sqlite_store_follows_the_identity_rules() {
    let test_database = TestDatabase::migrated(MIGRATIONS).await;
    let database = test_database.database().clone();
    let events = EventLog::new(&database, ServiceName::new("panel-api").unwrap());
    let store = Arc::new(SqliteIdentityStore::new(&database, events));
    store.sync_roles().await.unwrap();
    // Built-in roles are rewritten, not duplicated.
    store.sync_roles().await.unwrap();
    assert_eq!(store.roles().await.unwrap().len(), 4);

    check(|| {
        let store = Arc::clone(&store);
        let database = database.clone();
        async move {
            sqlx::raw_sql(
                "DELETE FROM accounts; DELETE FROM outbox; DELETE FROM roles WHERE NOT built_in",
            )
            .execute(database.pool())
            .await
            .unwrap();
            Sqlite { store, database }
        }
    })
    .await;
}
