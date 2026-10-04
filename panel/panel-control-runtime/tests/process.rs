#![forbid(unsafe_code)]

use chrono::Utc;
use panel_control_runtime::{
    ControlPlaneProcess, DefaultAddresses, ProcessSettings, DATABASE_PASSWORD_ENV,
    DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventType, EventVersion, Principal, RequestId, RequestScope,
};
use panel_health::{HealthStatus, ServiceMode};
use panel_jetstream::{testing::TestBroker, JetStreamServiceRegistry};
use panel_platform::{RegistrationPolicy, ServiceDirectory, ServiceName};
use panel_postgres::{testing::TestDatabase, PgOutbox, SqlIdentifier};
use panel_service::{describe_peer, Environment};
use std::{collections::HashMap, ffi::OsString, future::Future, time::Duration};
use tonic::transport::Channel;

fn settings(values: &[(&str, &str)]) -> ProcessSettings {
    let values: HashMap<String, OsString> = values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
        .collect();
    ProcessSettings::read(
        &mut Environment::from_lookup(move |name| values.get(name).cloned()),
        DefaultAddresses {
            ops: "127.0.0.1:0".parse().unwrap(),
            grpc: "127.0.0.1:0".parse().unwrap(),
        },
    )
    .unwrap()
    .with_health_interval(Duration::from_millis(50))
}

async fn eventually<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..200 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting until {what}");
}

fn event() -> EventEnvelope {
    EventEnvelope::new(
        EventDraft::new(
            EventType::new("config.revision.changed").unwrap(),
            EventVersion::V1,
            AggregateRef::new(
                AggregateType::new("revision").unwrap(),
                AggregateId::new("r1").unwrap(),
            ),
            EventPayload::json(&serde_json::json!({ "revision": 1 })).unwrap(),
        ),
        EventOrigin::scoped(
            ServiceName::new("config-service").unwrap(),
            &RequestScope::new(RequestId::new("req-1").unwrap()),
            Principal::system(Actor::new("config-service").unwrap()),
        ),
        Utc::now(),
    )
}

#[tokio::test]
async fn a_ready_process_serves_registers_relays_and_deregisters() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database.bootstrap(&[("config", "config")]).await;
    let nats_url = std::env::var(panel_jetstream::testing::NATS_URL_ENV).unwrap();
    let process = ControlPlaneProcess::new(
        ServiceName::new("config-service").unwrap(),
        "0.1.0-test",
        settings(&[
            (DATABASE_URL_ENV, &database.service_url("config")),
            (DATABASE_PASSWORD_ENV, secrets[0].expose()),
            (NATS_URL_ENV, &nats_url),
        ]),
        SqlIdentifier::new("config").unwrap(),
    )
    .unwrap()
    .with_jetstream_settings((*broker.settings).clone())
    .with_registration(
        Duration::from_secs(5),
        RegistrationPolicy::new(Duration::from_millis(100), Duration::from_millis(50)).unwrap(),
    )
    .start()
    .await
    .unwrap();

    let health = process.health();
    eventually("the process is ready", || {
        let health = health.clone();
        async move { health.current().status() == HealthStatus::Pass }
    })
    .await;
    let readiness: serde_json::Value = get(format!("http://{}/readyz", process.ops_address()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(readiness["status"], "pass");
    assert_eq!(readiness["serviceId"], "config-service");
    let metrics = get(format!("http://{}/metrics", process.ops_address()))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("pingora_panel_service_ready 1\n"),
        "{metrics}"
    );
    for check in ["schema", "postgresql", "nats"] {
        assert_eq!(
            readiness["checks"][format!("{check}:responseTime")][0]["status"],
            "pass"
        );
    }

    let channel = Channel::from_shared(format!("http://{}", process.grpc_address()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let described = describe_peer(channel, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(&described, process.descriptor());
    assert_eq!(described.schema_version(), "2");

    let registry = JetStreamServiceRegistry::provision(
        &broker.context,
        &broker.settings,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    eventually("the instance is registered", || async {
        registry.list().await.unwrap().services() == std::slice::from_ref(process.descriptor())
    })
    .await;

    let mut transaction = process.database().pool().begin().await.unwrap();
    PgOutbox::append(&mut transaction, &event()).await.unwrap();
    transaction.commit().await.unwrap();
    let outbox = PgOutbox::new(process.database());
    eventually("the relay publishes the event", || async {
        outbox.backlog().await.unwrap().pending == 0
    })
    .await;

    let ops = process.ops_address();
    process.stop().await;
    assert!(registry.list().await.unwrap().services().is_empty());
    assert!(tokio::net::TcpStream::connect(ops).await.is_err());

    broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await
        .unwrap();
    database.drop().await;
    broker.drop().await;
}

#[tokio::test]
async fn an_unreachable_database_keeps_the_process_unavailable_but_stoppable() {
    let process = ControlPlaneProcess::new(
        ServiceName::new("observability-service").unwrap(),
        "0.1.0-test",
        settings(&[
            (
                DATABASE_URL_ENV,
                "postgres://observability@127.0.0.1:1/panel",
            ),
            (NATS_URL_ENV, "nats://127.0.0.1:1"),
        ]),
        SqlIdentifier::new("observability").unwrap(),
    )
    .unwrap()
    .start()
    .await
    .unwrap();

    let health = process.health();
    eventually("checks have run", || {
        let health = health.clone();
        async move {
            health
                .current()
                .checks()
                .contains_key("schema:responseTime")
        }
    })
    .await;
    let report = health.current();
    assert_eq!(report.status(), HealthStatus::Fail);
    assert_eq!(report.mode(), ServiceMode::Unavailable);
    let response = get(format!("http://{}/readyz", process.ops_address())).await;
    assert_eq!(response.status(), 503);
    let liveness = get(format!("http://{}/livez", process.ops_address())).await;
    assert_eq!(liveness.status(), 200);

    tokio::time::timeout(Duration::from_secs(10), process.stop())
        .await
        .expect("stopping does not wait for unreachable dependencies");
}

#[cfg(unix)]
#[tokio::test]
async fn socket_peers_are_never_reached_over_plaintext() {
    let process = ControlPlaneProcess::new(
        ServiceName::new("panel-api").unwrap(),
        "0.1.0-test",
        settings(&[
            (DATABASE_URL_ENV, "postgres://panel@127.0.0.1:1/panel"),
            (NATS_URL_ENV, "nats://127.0.0.1:1"),
        ]),
        SqlIdentifier::new("identity").unwrap(),
    )
    .unwrap();
    let refused = process
        .peer_unix_channel(
            std::path::Path::new("/run/pingora-panel-ops/agent.sock"),
            ServiceName::new("ops-agent").unwrap(),
        )
        .unwrap_err();
    assert!(
        refused.message.contains("PINGORA_PANEL_TLS_DIR"),
        "{refused}"
    );
}

/// GETs `url` once the panel's ring provider is installed, which reqwest
/// needs to set up TLS even for plain HTTP.
async fn get(url: String) -> reqwest::Response {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::get(url).await.unwrap()
}
