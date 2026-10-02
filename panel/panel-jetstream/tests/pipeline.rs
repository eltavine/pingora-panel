#![forbid(unsafe_code)]

mod support;

use panel_events::{
    ConsumerName, EventOrigin, HandlerOutcome, IdempotentEventHandler, ServiceName,
};
use panel_jetstream::{testing::TestBroker, ConsumerSpec, JetStreamConsumer, JetStreamPublisher};
use panel_outbox::{OutboxRelay, RelayOptions};
use panel_postgres::{testing::TestDatabase, PgOutbox, PgProcessedEventStore};
use std::{sync::Arc, time::Duration};
use support::{event, ScriptedHandler};
use tokio::sync::oneshot;

/// A committed state change reaches a consumer exactly once, with its causal
/// identity intact, through the outbox, the relay and JetStream.
#[tokio::test]
async fn committed_events_flow_from_the_outbox_to_an_idempotent_consumer() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database
        .bootstrap(&[("config", "config"), ("automation", "automation")])
        .await;
    let producer = database
        .connect_service("config", "config", &secrets[0])
        .await;
    let consumer_db = database
        .connect_service("automation", "automation", &secrets[1])
        .await;
    producer.migrate(&[]).await.unwrap();
    consumer_db.migrate(&[]).await.unwrap();

    let outbox = PgOutbox::new(&producer);
    let leadership = outbox.try_lead().await.unwrap().expect("relay leads");
    let relay = OutboxRelay::new(
        Arc::new(outbox.clone()),
        Arc::new(JetStreamPublisher::new(
            broker.context.clone(),
            Arc::clone(&broker.settings),
        )),
        Arc::new(outbox.listen().await.unwrap()),
        RelayOptions::default().with_idle_poll(Duration::from_millis(500)),
    );
    let (stop_relay, relay_stopped) = oneshot::channel::<()>();
    let relay_task = tokio::spawn(relay.run(async move {
        let _ = relay_stopped.await;
    }));

    let recorder = ScriptedHandler::new(Vec::new(), HandlerOutcome::Ack);
    let handler = Arc::new(IdempotentEventHandler::new(
        Arc::clone(&recorder),
        Arc::new(PgProcessedEventStore::new(&consumer_db)),
        Duration::from_secs(30),
    ));
    let consumer = JetStreamConsumer::ensure(
        &broker.context,
        Arc::clone(&broker.settings),
        ConsumerSpec::new(
            ConsumerName::new("renewal").unwrap(),
            vec!["config.>".into()],
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let (stop_consumer, consumer_stopped) = oneshot::channel::<()>();
    let consumer_task = tokio::spawn(async move {
        consumer
            .run(handler, async move {
                let _ = consumer_stopped.await;
            })
            .await
            .unwrap();
    });

    let original = event("config.revision.activated", 1);
    let follow_up = panel_events::EventEnvelope::new(
        panel_events::EventDraft::new(
            panel_events::EventType::new("config.revision.superseded").unwrap(),
            panel_events::EventVersion::V1,
            original.aggregate().clone(),
            panel_events::EventPayload::json(&serde_json::json!({ "revision": 41 })).unwrap(),
        ),
        EventOrigin::caused_by(ServiceName::new("config-service").unwrap(), &original),
        chrono::Utc::now(),
    );
    let mut transaction = producer.pool().begin().await.unwrap();
    PgOutbox::append(&mut transaction, &original).await.unwrap();
    PgOutbox::append(&mut transaction, &follow_up)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    recorder.wait_for(2).await;
    let deliveries = recorder.deliveries();
    assert_eq!(deliveries[0].envelope(), &original);
    assert_eq!(deliveries[1].envelope(), &follow_up);
    assert_eq!(
        deliveries[1].envelope().correlation_id(),
        original.correlation_id()
    );
    assert_eq!(
        deliveries[1].envelope().causation_id().as_str(),
        original.event_id().to_string()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while outbox.backlog().await.unwrap().pending > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the relay marks published events");

    let _ = stop_relay.send(());
    let _ = stop_consumer.send(());
    relay_task.await.unwrap();
    consumer_task.await.unwrap();
    leadership.release().await.unwrap();
    producer.close().await;
    consumer_db.close().await;
    database.drop().await;
    broker.drop().await;
}
