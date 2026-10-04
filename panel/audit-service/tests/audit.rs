#![forbid(unsafe_code)]

use audit_service::SqliteAuditStore;
use chrono::Utc;
use panel_contracts::audit::v1::{
    audit_query_client::AuditQueryClient, ListRequest, VerifyRequest,
};
use panel_control_runtime::{ProcessSettings, NATS_URL_ENV};
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventPublisher, EventType, EventVersion, Principal, RequestId, RequestScope,
    ServiceName,
};
use panel_health::HealthStatus;
use panel_jetstream::{
    testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV},
    JetStreamPublisher,
};
use panel_service::Environment;
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Duration};

fn event(event_type: &str, actor: &str, request: &str, data: Value) -> EventEnvelope {
    EventEnvelope::new(
        EventDraft::new(
            EventType::new(event_type).unwrap(),
            EventVersion::V1,
            AggregateRef::new(
                AggregateType::new("configuration").unwrap(),
                AggregateId::new("draft").unwrap(),
            ),
            EventPayload::json(&data).unwrap(),
        ),
        EventOrigin::scoped(
            ServiceName::new("config-service").unwrap(),
            &RequestScope::new(RequestId::new(request).unwrap()),
            Principal::user(Actor::new(actor).unwrap()),
        ),
        Utc::now(),
    )
}

#[tokio::test]
async fn every_event_is_recorded_once_in_a_verifiable_chain() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let values: HashMap<&str, OsString> = HashMap::from([(
        NATS_URL_ENV,
        std::env::var(TEST_NATS_URL_ENV).unwrap().into(),
    )]);
    let mut env = Environment::from_lookup(move |name| values.get(name).cloned());
    let settings = ProcessSettings::read(&mut env, audit_service::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50))
        .with_data_directory(directory.path());
    let process = audit_service::process(&mut env, settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().status() != HealthStatus::Pass {
            assert!(health.changed().await);
        }
    })
    .await
    .expect("audit-service becomes ready");

    let events = [
        event(
            "config.draft.changed",
            "ops",
            "req-1",
            json!({"version": 1, "operation": "sites.create", "resource": "sites"}),
        ),
        event(
            "config.draft.applied",
            "ops",
            "req-2",
            json!({"version": 1, "revision": 1}),
        ),
        event(
            "gateway.reloaded",
            "admin",
            "req-3",
            json!({"generation": 2}),
        ),
    ];
    let publisher = JetStreamPublisher::new(broker.context.clone(), Arc::clone(&broker.settings));
    for event in &events {
        publisher.publish(event).await.unwrap();
    }

    let mut client = AuditQueryClient::connect(format!("http://{}", process.grpc_address()))
        .await
        .unwrap();
    let list = |request: ListRequest| {
        let mut client = client.clone();
        async move { client.list(request).await.unwrap().into_inner() }
    };
    let records = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let page = list(ListRequest::default()).await;
            if page.records.len() == 3 {
                return page.records;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("events are recorded");
    assert_eq!(
        records
            .iter()
            .map(|record| (record.sequence, record.event_type.as_str()))
            .collect::<Vec<_>>(),
        [
            (3, "gateway.reloaded"),
            (2, "config.draft.applied"),
            (1, "config.draft.changed")
        ]
    );
    assert_eq!(records[2].actor_id, "ops");
    assert_eq!(records[2].actor_type, "user");
    assert_eq!(records[2].correlation_id, "req-1");
    assert_eq!(records[2].subject, "configuration/draft");
    assert_eq!(records[2].previous_hash, "");
    assert_eq!(records[1].previous_hash, records[2].hash);
    let data: Value = serde_json::from_slice(&records[2].data).unwrap();
    assert_eq!(data["operation"], "sites.create");

    let by_actor = list(ListRequest {
        actor_id: "ops".into(),
        ..ListRequest::default()
    })
    .await;
    assert_eq!(by_actor.records.len(), 2);
    let by_prefix = list(ListRequest {
        event_type: "config.".into(),
        limit: 1,
        ..ListRequest::default()
    })
    .await;
    assert_eq!(by_prefix.records[0].event_type, "config.draft.applied");
    assert_eq!(by_prefix.next_before, Some(2));
    let older = list(ListRequest {
        event_type: "config.".into(),
        before: by_prefix.next_before,
        ..ListRequest::default()
    })
    .await;
    assert_eq!(older.records[0].sequence, 1);
    let by_correlation = list(ListRequest {
        correlation_id: "req-3".into(),
        ..ListRequest::default()
    })
    .await;
    assert_eq!(by_correlation.records[0].event_type, "gateway.reloaded");

    let store = SqliteAuditStore::new(process.database());
    assert_eq!(store.append(&events[0]).await.unwrap(), 1);
    assert_eq!(list(ListRequest::default()).await.records.len(), 3);

    let verified = client
        .verify(VerifyRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(verified.intact);
    assert_eq!(verified.checked, 3);
    assert_eq!(verified.head_sequence, 3);
    assert_eq!(verified.head_hash, records[0].hash);

    let pool = process.database().pool();
    for statement in [
        "UPDATE audit_records SET actor_id = 'mallory' WHERE sequence = 2",
        "DELETE FROM audit_records WHERE sequence = 2",
    ] {
        let refused = sqlx::query(statement).execute(pool).await.unwrap_err();
        assert!(refused.to_string().contains("append-only"), "{refused}");
    }
    for statement in [
        "DROP TRIGGER audit_records_no_update",
        "UPDATE audit_records SET actor_id = 'mallory' WHERE sequence = 2",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    let tampered = client
        .verify(VerifyRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(!tampered.intact);
    assert_eq!(tampered.first_mismatch, Some(2));
    assert_eq!(tampered.checked, 1);

    process.stop().await;
    broker.drop().await;
}
