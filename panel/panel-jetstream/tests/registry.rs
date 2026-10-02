#![forbid(unsafe_code)]

use chrono::Utc;
use panel_jetstream::{testing::TestBroker, JetStreamServiceRegistry};
use panel_platform::{
    Capability, ProtocolRange, ServiceDescriptor, ServiceDirectory, ServiceName, ServiceRegistrar,
};
use std::time::Duration;

fn instance(service: &str, capability: &str) -> ServiceDescriptor {
    ServiceDescriptor::new(ServiceName::new(service).unwrap(), "0.1.0", Utc::now())
        .with_protocol(ProtocolRange::up_to("pingora.panel.platform.v1", 1).unwrap())
        .with_capability(Capability::new(capability, "1").unwrap())
}

#[tokio::test]
async fn registrations_are_listed_until_deregistered_or_expired() {
    let Some(broker) = TestBroker::create().await else {
        return;
    };
    let registry = JetStreamServiceRegistry::provision(
        &broker.context,
        &broker.settings,
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let config = instance("config-service", "revision.plan");
    let automation = instance("automation-service", "job.schedule");
    registry.register(&config).await.unwrap();
    registry.register(&automation).await.unwrap();
    registry.register(&config).await.unwrap();

    let listing = registry.list().await.unwrap();
    assert_eq!(listing.services(), [automation.clone(), config.clone()]);
    assert_eq!(
        listing
            .providers("revision.plan")
            .map(|descriptor| descriptor.instance_id())
            .collect::<Vec<_>>(),
        [config.instance_id()]
    );
    assert!(listing.observed_at() <= Utc::now());

    registry.deregister(&automation).await.unwrap();
    assert_eq!(
        registry.list().await.unwrap().services(),
        std::slice::from_ref(&config)
    );

    let mut expired = false;
    for _ in 0..40 {
        if registry.list().await.unwrap().services().is_empty() {
            expired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(expired, "an unrefreshed registration expires");

    let reprovisioned = JetStreamServiceRegistry::provision(
        &broker.context,
        &broker.settings,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    reprovisioned.register(&config).await.unwrap();
    assert_eq!(reprovisioned.list().await.unwrap().services().len(), 1);

    broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await
        .unwrap();
    broker.drop().await;
}
