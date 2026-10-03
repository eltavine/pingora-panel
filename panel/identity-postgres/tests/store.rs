#![forbid(unsafe_code)]

use async_trait::async_trait;
use identity_postgres::{PgIdentityStore, MIGRATIONS};
use panel_events::ServiceName;
use panel_identity::{
    conformance::{check, StoreUnderTest},
    memory::RecordedEvent,
    IdentityStore, ProviderStore, WorkloadStore,
};
use panel_postgres::{testing::TestDatabase, EventLog, ServiceDatabase};
use std::sync::Arc;

struct Postgres {
    store: Arc<PgIdentityStore>,
    database: ServiceDatabase,
}

#[async_trait]
impl StoreUnderTest for Postgres {
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
async fn the_postgres_store_follows_the_identity_rules() {
    let Some(mut test_database) = TestDatabase::create().await else {
        return;
    };
    let secrets = test_database.bootstrap(&[("identity", "identity")]).await;
    let database = test_database
        .connect_service("identity", "identity", &secrets[0])
        .await;
    database.migrate(MIGRATIONS).await.unwrap();
    let events = EventLog::new(&database, ServiceName::new("panel-api").unwrap());
    let store = Arc::new(PgIdentityStore::new(&database, events));
    store.sync_roles().await.unwrap();
    // Built-in roles are rewritten, not duplicated.
    store.sync_roles().await.unwrap();
    assert_eq!(store.roles().await.unwrap().len(), 4);

    check(|| {
        let store = Arc::clone(&store);
        let database = database.clone();
        async move {
            sqlx::raw_sql(
                "TRUNCATE accounts, outbox CASCADE; DELETE FROM roles WHERE NOT built_in",
            )
            .execute(database.pool())
            .await
            .unwrap();
            Postgres { store, database }
        }
    })
    .await;
    test_database.drop().await;
}
