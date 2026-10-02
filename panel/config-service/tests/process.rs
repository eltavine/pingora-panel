#![forbid(unsafe_code)]

use panel_control_runtime::{
    ProcessSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_health::{HealthStatus, ServiceMode};
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_service::Environment;
use std::{collections::HashMap, ffi::OsString, time::Duration};

fn environment(values: Vec<(&'static str, String)>) -> Environment<'static> {
    let values: HashMap<&str, OsString> = values
        .into_iter()
        .map(|(key, value)| (key, OsString::from(value)))
        .collect();
    Environment::from_lookup(move |name| values.get(name).cloned())
}

#[tokio::test]
async fn plaintext_gateway_connections_must_stay_on_loopback() {
    let mut env = environment(vec![
        (DATABASE_URL_ENV, "postgres://config@127.0.0.1/panel".into()),
        (
            config_service::GATEWAY_URL_ENV,
            "http://192.0.2.10:50051".into(),
        ),
    ]);
    let settings = ProcessSettings::read(&mut env, config_service::default_addresses()).unwrap();
    assert!(config_service::process(&mut env, settings).is_err());
}

/// Without a gateway the service still serves reads: it runs degraded.
#[tokio::test]
async fn an_unreachable_gateway_degrades_the_service() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let mut env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("config")),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
        (NATS_URL_ENV, std::env::var(TEST_NATS_URL_ENV).unwrap()),
        (config_service::GATEWAY_URL_ENV, "http://127.0.0.1:1".into()),
    ]);
    let settings = ProcessSettings::read(&mut env, config_service::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50));
    let process = config_service::process(&mut env, settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();

    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().mode() != ServiceMode::Degraded {
            assert!(health.changed().await);
        }
    })
    .await
    .expect("the service reports itself degraded");
    let report = health.current();
    assert_eq!(report.status(), HealthStatus::Warn);
    assert_eq!(
        report.checks()["gatewayd:responseTime"][0].status(),
        HealthStatus::Fail
    );
    assert_eq!(
        report.checks()["schema:responseTime"][0].status(),
        HealthStatus::Pass
    );

    process.stop().await;
    let _ = broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await;
    database.drop().await;
    broker.drop().await;
}
