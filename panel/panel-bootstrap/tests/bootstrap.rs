#![forbid(unsafe_code)]

use panel_bootstrap::Plan;
use panel_control_runtime::NATS_URL_ENV;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_service::Environment;
use std::ffi::OsString;

#[tokio::test]
async fn provisioning_is_repeatable() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let nats = std::env::var(TEST_NATS_URL_ENV).unwrap();
    let plan = Plan::read(&mut Environment::from_lookup(move |name| {
        (name == NATS_URL_ENV).then(|| OsString::from(&nats))
    }))
    .unwrap()
    .with_jetstream_settings((*broker.settings).clone());

    plan.apply().await.unwrap();
    plan.apply().await.unwrap();

    for stream in [
        broker.settings.events_stream(),
        broker.settings.dead_letter_stream(),
    ] {
        broker.context.get_stream(stream).await.unwrap();
    }
    broker
        .context
        .get_key_value(broker.settings.service_bucket())
        .await
        .unwrap();

    broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await
        .unwrap();
    broker.drop().await;
}
