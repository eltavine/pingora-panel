#![forbid(unsafe_code)]

use panel_postgres::{
    storage_error, testing::TestDatabase, DatabaseBootstrap, RoleSecret, ScramVerifier,
    ServiceDatabase, ServiceRole, SqlIdentifier,
};
use sqlx::AssertSqlSafe;

const SERVICES: &[(&str, &str)] = &[("config", "config"), ("automation", "automation")];

async fn denied(database: &ServiceDatabase, statement: &str) -> bool {
    match sqlx::raw_sql(AssertSqlSafe(statement.to_owned()))
        .execute(database.pool())
        .await
    {
        Ok(_) => false,
        Err(error) => storage_error(error).code.as_str() == "PERMISSION_DENIED",
    }
}

#[tokio::test]
async fn service_roles_write_only_their_own_schema() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let secrets = database.bootstrap(SERVICES).await;
    let config = database
        .connect_service("config", "config", &secrets[0])
        .await;
    let automation = database
        .connect_service("automation", "automation", &secrets[1])
        .await;

    sqlx::query("CREATE TABLE revisions (id BIGINT PRIMARY KEY)")
        .execute(config.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)")
        .execute(config.pool())
        .await
        .unwrap();
    assert_eq!(config.schema().as_str(), "config");

    // Unqualified names resolve only within the caller's own schema.
    assert!(sqlx::query("SELECT id FROM revisions")
        .fetch_all(automation.pool())
        .await
        .is_err());
    for statement in [
        "SELECT id FROM config.revisions",
        "INSERT INTO config.revisions VALUES (2)",
        "DELETE FROM config.revisions",
        "CREATE TABLE config.injected (id INT)",
        "CREATE TABLE public.leaked (id INT)",
        "CREATE SCHEMA rogue",
    ] {
        assert!(
            denied(&automation, statement).await,
            "{statement} must be denied"
        );
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM revisions")
        .fetch_one(config.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);

    config.close().await;
    automation.close().await;
    database.drop().await;
}

#[tokio::test]
async fn bootstrap_is_idempotent_and_rotates_credentials() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let first = database.bootstrap(&SERVICES[..1]).await;
    let original = database
        .connect_service("config", "config", &first[0])
        .await;
    sqlx::query("CREATE TABLE kept (id INT)")
        .execute(original.pool())
        .await
        .unwrap();
    original.close().await;

    let rotated = RoleSecret::generate().unwrap();
    DatabaseBootstrap::new()
        .with_service(ServiceRole::new(
            database.role_name("config"),
            SqlIdentifier::new("config").unwrap(),
            ScramVerifier::derive(&rotated).unwrap(),
        ))
        .apply(&mut database.admin_connection().await)
        .await
        .unwrap();

    // The client-side SCRAM verifier authenticates the new secret, the old
    // secret no longer works, and existing objects survive the rerun.
    let error = ServiceDatabase::connect(database.service_config("config", "config", &first[0]))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "UNAUTHENTICATED");
    let reconnected = database.connect_service("config", "config", &rotated).await;
    sqlx::query("SELECT id FROM kept")
        .fetch_all(reconnected.pool())
        .await
        .unwrap();
    reconnected.close().await;
    database.drop().await;
}

#[tokio::test]
async fn bootstrap_refuses_a_service_owned_database() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    database.bootstrap(&SERVICES[..1]).await;
    let mut admin = database.admin_connection().await;
    sqlx::raw_sql(AssertSqlSafe(format!(
        "ALTER DATABASE {} OWNER TO {}",
        sqlx::query_scalar::<_, String>("SELECT current_database()::text")
            .fetch_one(&mut admin)
            .await
            .unwrap(),
        database.role_name("config")
    )))
    .execute(&mut admin)
    .await
    .unwrap();

    let error = DatabaseBootstrap::new()
        .with_service(ServiceRole::new(
            database.role_name("config"),
            SqlIdentifier::new("config").unwrap(),
            ScramVerifier::derive(&RoleSecret::generate().unwrap()).unwrap(),
        ))
        .apply(&mut admin)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "PRECONDITION_FAILED");
    drop(admin);
    database.drop().await;
}
