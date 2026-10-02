#![forbid(unsafe_code)]

mod support;

use async_nats::jetstream::message::PublishMessage;
use panel_event_codec::{json, CLOUDEVENTS_JSON_MEDIA_TYPE};
use panel_events::{
    ConsumerName, EventHandler, EventPublisher, HandlerOutcome, IdempotentEventHandler,
    MemoryProcessedEventStore,
};
use panel_jetstream::{
    ensure_streams, testing::TestBroker, ConsumerSpec, DeadLetterQueue, JetStreamConsumer,
    JetStreamPublisher, StreamChange,
};
use std::{num::NonZeroU32, sync::Arc, time::Duration};
use support::{event, ScriptedHandler};
use tokio::sync::oneshot;

struct Running {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("consumer stops")
            .unwrap();
    }
}

async fn consume(
    broker: &TestBroker,
    spec: ConsumerSpec,
    handler: Arc<dyn EventHandler>,
) -> Running {
    let consumer = JetStreamConsumer::ensure(&broker.context, Arc::clone(&broker.settings), spec)
        .await
        .unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        consumer
            .run(handler, async move {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    Running { stop, task }
}

fn spec(name: &str) -> ConsumerSpec {
    ConsumerSpec::new(ConsumerName::new(name).unwrap(), vec!["config.>".into()])
        .unwrap()
        .with_ack_wait(Duration::from_secs(5))
        .with_max_deliver(NonZeroU32::new(3).unwrap())
}

#[tokio::test]
async fn streams_are_provisioned_idempotently() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let changes = ensure_streams(&broker.context, &broker.settings)
        .await
        .unwrap();
    assert!(changes
        .iter()
        .all(|(_, change)| *change == StreamChange::Unchanged));

    let shorter = (*broker.settings)
        .clone()
        .with_retention(Duration::from_secs(3600), Duration::from_secs(7200));
    let changes = ensure_streams(&broker.context, &shorter).await.unwrap();
    assert!(changes
        .iter()
        .all(|(_, change)| *change == StreamChange::Updated));
    broker.drop().await;
}

#[tokio::test]
async fn publication_is_deduplicated_by_event_id() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let publisher = JetStreamPublisher::new(broker.context.clone(), Arc::clone(&broker.settings));
    let event = event("config.revision.activated", 1);

    let first = publisher.publish(&event).await.unwrap();
    let repeated = publisher.publish(&event).await.unwrap();
    assert!(!first.duplicate());
    assert!(repeated.duplicate());
    assert_eq!(first.sequence(), repeated.sequence());
    broker.drop().await;
}

#[tokio::test]
async fn consumers_ack_retry_and_park_events() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let publisher = JetStreamPublisher::new(broker.context.clone(), Arc::clone(&broker.settings));
    let handler = ScriptedHandler::new(
        vec![
            HandlerOutcome::retry(Duration::from_millis(100), "warming up"),
            HandlerOutcome::Ack,
            HandlerOutcome::dead_letter("unsupported revision"),
        ],
        HandlerOutcome::retry(Duration::from_millis(100), "still failing"),
    );
    let running = consume(&broker, spec("projection"), handler.clone()).await;

    let retried = event("config.revision.activated", 1);
    publisher.publish(&retried).await.unwrap();
    handler.wait_for(2).await;
    let deliveries = handler.deliveries();
    assert_eq!(deliveries[0].envelope(), &retried);
    assert_eq!(deliveries[0].attempt().get(), 1);
    assert_eq!(deliveries[1].attempt().get(), 2);

    let parked = event("config.revision.activated", 2);
    publisher.publish(&parked).await.unwrap();
    handler.wait_for(3).await;

    let exhausted = event("config.revision.activated", 3);
    publisher.publish(&exhausted).await.unwrap();
    handler.wait_for(6).await;
    running.stop().await;

    let dead_letters = DeadLetterQueue::new(broker.context.clone(), Arc::clone(&broker.settings));
    let consumer = ConsumerName::new("projection").unwrap();
    let records = dead_letters.list(&consumer, 0, 10).await.unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].envelope.as_ref(), Some(&parked));
    assert_eq!(records[0].reason, "unsupported revision");
    assert_eq!(records[0].deliveries, 1);
    assert_eq!(records[1].envelope.as_ref(), Some(&exhausted));
    assert_eq!(records[1].reason, "retries exhausted: still failing");
    assert_eq!(records[1].deliveries, 3);
    assert_eq!(records[0].consumer.as_ref(), Some(&consumer));
    broker.drop().await;
}

#[tokio::test]
async fn replay_redelivers_only_to_the_consumer_that_failed() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let publisher = JetStreamPublisher::new(broker.context.clone(), Arc::clone(&broker.settings));
    let failing = ScriptedHandler::new(
        vec![HandlerOutcome::dead_letter("dependency missing")],
        HandlerOutcome::Ack,
    );
    let other = ScriptedHandler::new(Vec::new(), HandlerOutcome::Ack);
    let failing_run = consume(&broker, spec("failing"), failing.clone()).await;
    let other_run = consume(&broker, spec("other"), other.clone()).await;

    let original = event("config.revision.activated", 7);
    publisher.publish(&original).await.unwrap();
    failing.wait_for(1).await;
    other.wait_for(1).await;

    let dead_letters = DeadLetterQueue::new(broker.context.clone(), Arc::clone(&broker.settings));
    let consumer = ConsumerName::new("failing").unwrap();
    let parked = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let records = dead_letters.list(&consumer, 0, 10).await.unwrap();
            if let Some(record) = records.into_iter().next() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();

    let receipt = dead_letters.replay(parked.sequence).await.unwrap();
    assert_eq!(receipt.dead_letter_sequence, parked.sequence);
    failing.wait_for(2).await;
    assert_eq!(failing.deliveries()[1].envelope(), &original);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        other.deliveries().len(),
        1,
        "replay must not reach other consumers"
    );
    assert!(dead_letters
        .list(&consumer, 0, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        dead_letters
            .replay(parked.sequence)
            .await
            .unwrap_err()
            .code
            .as_str(),
        "NOT_FOUND"
    );

    failing_run.stop().await;
    other_run.stop().await;
    broker.drop().await;
}

#[tokio::test]
async fn deliveries_exhausted_without_a_decision_are_parked_from_the_advisory() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let consumer_name = ConsumerName::new("crashing").unwrap();
    let spec = spec("crashing")
        .with_ack_wait(Duration::from_secs(1))
        .with_max_deliver(NonZeroU32::MIN);
    JetStreamConsumer::ensure(&broker.context, Arc::clone(&broker.settings), spec)
        .await
        .unwrap();
    let dead_letters = DeadLetterQueue::new(broker.context.clone(), Arc::clone(&broker.settings));
    let (stop, stopped) = oneshot::channel::<()>();
    let watcher = {
        let dead_letters = dead_letters.clone();
        let consumer_name = consumer_name.clone();
        tokio::spawn(async move {
            dead_letters
                .watch_max_deliveries(&consumer_name, async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;

    let publisher = JetStreamPublisher::new(broker.context.clone(), Arc::clone(&broker.settings));
    let lost = event("config.revision.activated", 11);
    publisher.publish(&lost).await.unwrap();

    // A worker takes the only delivery and dies without answering it.
    let stream = broker
        .context
        .get_stream(broker.settings.events_stream())
        .await
        .unwrap();
    let raw: async_nats::jetstream::consumer::PullConsumer =
        stream.get_consumer("crashing").await.unwrap();
    let mut batch = raw.fetch().max_messages(1).messages().await.unwrap();
    use futures_util::StreamExt;
    let taken = batch.next().await.unwrap().unwrap();
    drop(taken);

    let record = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(record) = dead_letters
                .list(&consumer_name, 0, 10)
                .await
                .unwrap()
                .into_iter()
                .next()
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the advisory parks the lost delivery");
    assert_eq!(record.envelope.as_ref(), Some(&lost));
    assert!(record.reason.starts_with("maximum deliveries exceeded"));

    let _ = stop.send(());
    watcher.await.unwrap();
    broker.drop().await;
}

#[tokio::test]
async fn structured_json_messages_and_duplicates_are_handled_once() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let recorder = ScriptedHandler::new(Vec::new(), HandlerOutcome::Ack);
    let store = Arc::new(MemoryProcessedEventStore::new());
    let handler = Arc::new(IdempotentEventHandler::new(
        Arc::clone(&recorder),
        store,
        Duration::from_secs(30),
    ));
    let running = consume(&broker, spec("idempotent"), handler).await;

    // The same event arrives twice in structured mode under different broker
    // message IDs, as after a relay restart outside the duplicate window.
    let event = event("config.revision.activated", 9);
    let subject = broker
        .settings
        .event_subject(event.event_type(), event.event_version());
    for attempt in 0..2 {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Content-Type", CLOUDEVENTS_JSON_MEDIA_TYPE);
        broker
            .context
            .send_publish(
                subject.clone(),
                PublishMessage::build()
                    .headers(headers)
                    .payload(json::encode(&event).unwrap().into())
                    .message_id(format!("structured-{attempt}")),
            )
            .await
            .unwrap()
            .await
            .unwrap();
    }
    recorder.wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(recorder.deliveries().len(), 1);
    assert_eq!(recorder.deliveries()[0].envelope(), &event);

    running.stop().await;
    broker.drop().await;
}
