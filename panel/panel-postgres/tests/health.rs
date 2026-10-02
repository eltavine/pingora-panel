#![forbid(unsafe_code)]

use panel_health::{HealthCheck, HealthStatus};
use panel_postgres::{testing::TestDatabase, PgHealthCheck};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[tokio::test]
async fn passes_while_the_service_database_answers() {
    let Some(mut database) = TestDatabase::create().await else {
        return;
    };
    let secrets = database
        .bootstrap(&[("observability", "observability")])
        .await;
    let service = database
        .connect_service("observability", "observability", &secrets[0])
        .await;
    let check = PgHealthCheck::new(service.pool().clone());
    assert_eq!(check.check().await.status(), HealthStatus::Pass);

    service.close().await;
    let closed = check.check().await;
    assert_eq!(closed.status(), HealthStatus::Fail);
    assert_eq!(closed.output(), Some("pool closed"));
    database.drop().await;
}

#[tokio::test]
async fn unreachable_servers_fail_without_exposing_driver_errors() {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://panel:secret@127.0.0.1:1/panel")
        .unwrap();
    let outcome = PgHealthCheck::new(pool).check().await;
    assert_eq!(outcome.status(), HealthStatus::Fail);
    assert_eq!(outcome.output(), Some("query failed"));
}
