#![forbid(unsafe_code)]

use panel_health::{HealthCheck, HealthStatus};
use panel_sqlite::{
    testing::TestDatabase, SchemaMigration, ServiceDatabase, ServiceDatabaseConfig,
    SqliteHealthCheck,
};
use std::sync::Arc;

const COUNTER: &[SchemaMigration] = &[SchemaMigration::new(
    SchemaMigration::SERVICE_VERSION_FLOOR,
    "counter",
    "CREATE TABLE counter (id INTEGER PRIMARY KEY, value INTEGER NOT NULL) STRICT;
     INSERT INTO counter VALUES (1, 0);",
)];

#[tokio::test]
async fn files_are_private_durable_and_migrate_once() {
    let test = TestDatabase::migrated(COUNTER).await;
    let database = test.database();
    database.migrate(COUNTER).await.unwrap();

    let pool = database.pool();
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(pool)
        .await
        .unwrap();
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(pool)
        .await
        .unwrap();
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!((journal.as_str(), synchronous, foreign_keys), ("wal", 2, 1));
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(applied, 3);
    assert_eq!(SchemaMigration::latest(COUNTER), 10_000);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(database.path()), 0o600);
        assert_eq!(mode(&database.path().with_extension("db-wal")), 0o600);
    }
}

const LINKED: &[SchemaMigration] = &[
    SchemaMigration::new(
        SchemaMigration::SERVICE_VERSION_FLOOR,
        "channels",
        "CREATE TABLE channels (id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('a'))) STRICT;
         CREATE TABLE sent (id INTEGER PRIMARY KEY,
             channel TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE) STRICT;
         INSERT INTO channels VALUES ('one', 'a');
         INSERT INTO sent (channel) VALUES ('one');",
    ),
    SchemaMigration::rebuilding(
        SchemaMigration::SERVICE_VERSION_FLOOR + 1,
        "channels of another kind",
        "PRAGMA foreign_keys = OFF;
         BEGIN IMMEDIATE;
         CREATE TABLE channels_next (id TEXT PRIMARY KEY,
             kind TEXT NOT NULL CHECK (kind IN ('a', 'b'))) STRICT;
         INSERT INTO channels_next SELECT id, kind FROM channels;
         DROP TABLE channels;
         ALTER TABLE channels_next RENAME TO channels;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    ),
];

#[tokio::test]
async fn rebuilt_tables_keep_what_references_them() {
    let test = TestDatabase::migrated(LINKED).await;
    let pool = test.database().pool();
    let sent: i64 = sqlx::query_scalar("SELECT count(*) FROM sent")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(sent, 1, "dropping the old table cascaded nothing");
    sqlx::query("INSERT INTO channels VALUES ('two', 'b')")
        .execute(pool)
        .await
        .unwrap();
    let broken = sqlx::query("INSERT INTO sent (channel) VALUES ('none')")
        .execute(pool)
        .await;
    assert!(broken.is_err(), "the references hold against the new table");
    sqlx::query("DELETE FROM channels WHERE id = 'one'")
        .execute(pool)
        .await
        .unwrap();
    let sent: i64 = sqlx::query_scalar("SELECT count(*) FROM sent")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(sent, 0, "and cascade from it");
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(foreign_keys, 1);
}

#[tokio::test]
async fn module_names_become_file_names() {
    let directory = tempfile::tempdir().unwrap();
    let config = ServiceDatabaseConfig::new(directory.path().join("control"), "config").unwrap();
    assert_eq!(config.path(), directory.path().join("control/config.db"));
    assert!(ServiceDatabaseConfig::new(directory.path(), "../config").is_err());
    assert!(ServiceDatabaseConfig::new(directory.path(), "").is_err());

    let database = ServiceDatabase::open(config).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.path().join("control"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    database.close().await;
}

#[tokio::test]
async fn misdeclared_migrations_are_refused() {
    let test = TestDatabase::migrated(&[]).await;
    let platform = [SchemaMigration::new(5, "too low", "SELECT 1")];
    assert!(test.database().migrate(&platform).await.is_err());
    let twice = [
        SchemaMigration::new(10_000, "one", "SELECT 1"),
        SchemaMigration::new(10_000, "two", "SELECT 1"),
    ];
    assert!(test.database().migrate(&twice).await.is_err());
}

#[tokio::test]
async fn concurrent_writers_take_turns_instead_of_failing() {
    let test = TestDatabase::migrated(COUNTER).await;
    let database = Arc::new(test.database().clone());
    let writers = (0..32)
        .map(|_| {
            let database = database.clone();
            tokio::spawn(async move {
                let mut transaction = database.begin().await.unwrap();
                let value: i64 = sqlx::query_scalar("SELECT value FROM counter WHERE id = 1")
                    .fetch_one(&mut *transaction)
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
                sqlx::query("UPDATE counter SET value = ?1 WHERE id = 1")
                    .bind(value + 1)
                    .execute(&mut *transaction)
                    .await
                    .unwrap();
                transaction.commit().await.unwrap();
            })
        })
        .collect::<Vec<_>>();
    for writer in writers {
        writer.await.unwrap();
    }
    let value: i64 = sqlx::query_scalar("SELECT value FROM counter WHERE id = 1")
        .fetch_one(database.pool())
        .await
        .unwrap();
    assert_eq!(value, 32);
}

#[tokio::test]
async fn constraint_violations_keep_their_meaning() {
    let test = TestDatabase::migrated(COUNTER).await;
    let duplicate = sqlx::query("INSERT INTO counter VALUES (1, 0)")
        .execute(test.database().pool())
        .await
        .map_err(panel_sqlite::storage_error)
        .unwrap_err();
    assert_eq!(duplicate.code.as_str(), "CONFLICT");
    let mistyped = sqlx::query("INSERT INTO counter VALUES (2, 'many')")
        .execute(test.database().pool())
        .await
        .map_err(panel_sqlite::storage_error)
        .unwrap_err();
    assert_eq!(mistyped.code.as_str(), "INVALID_ARGUMENT");
}

#[tokio::test]
async fn health_follows_the_pool() {
    let test = TestDatabase::migrated(&[]).await;
    let check = SqliteHealthCheck::new(test.database().pool().clone());
    assert_eq!(check.check().await.status(), HealthStatus::Pass);
    test.database().close().await;
    let closed = check.check().await;
    assert_eq!(closed.status(), HealthStatus::Fail);
    assert_eq!(closed.output(), Some("pool closed"));
}
