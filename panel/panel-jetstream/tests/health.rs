#![forbid(unsafe_code)]

use panel_health::{HealthCheck, HealthStatus};
use panel_jetstream::{testing::TestBroker, JetStreamHealthCheck};

#[tokio::test]
async fn passes_while_jetstream_answers() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let check = JetStreamHealthCheck::new(broker.context.clone());
    assert_eq!(check.check().await.status(), HealthStatus::Pass);
    broker.drop().await;
}

#[tokio::test]
async fn disconnected_clients_fail_without_waiting_for_a_request() {
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("nats://127.0.0.1:1")
        .await
        .unwrap();
    let outcome = JetStreamHealthCheck::new(async_nats::jetstream::new(client))
        .check()
        .await;
    assert_eq!(outcome.status(), HealthStatus::Fail);
    assert_eq!(outcome.output(), Some("not connected"));
}
