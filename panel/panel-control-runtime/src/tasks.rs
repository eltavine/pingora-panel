use crate::retry::until_success;
use async_nats::jetstream::Context;
use async_trait::async_trait;
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use panel_jetstream::{
    ensure_streams, JetStreamPublisher, JetStreamServiceRegistry, JetStreamSettings,
};
use panel_outbox::{OutboxRelay, RelayOptions};
use panel_platform::{maintain_registration, RegistrationPolicy, ServiceDescriptor};
use panel_postgres::{PgOutbox, SchemaMigration, ServiceDatabase};
use panel_sqlite::SqliteOutbox;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// How often a standby instance asks for outbox leadership.
const STANDBY_INTERVAL: Duration = Duration::from_secs(5);
/// How often a leader verifies that its leadership session is alive.
const LEADERSHIP_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Published outbox rows are kept this long for diagnosis, then purged.
const PUBLISHED_RETENTION: Duration = Duration::from_secs(24 * 3600);
const PURGE_INTERVAL: Duration = Duration::from_secs(600);
const PURGE_BATCH: i64 = 10_000;

/// Passes once the service schema has been verified and migrated.
pub(crate) struct SchemaCheck(pub(crate) watch::Receiver<bool>);

#[async_trait]
impl HealthCheck for SchemaCheck {
    fn component(&self) -> &str {
        "schema"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Datastore
    }

    async fn check(&self) -> CheckOutcome {
        if *self.0.borrow() {
            CheckOutcome::pass()
        } else {
            CheckOutcome::fail("not migrated yet")
        }
    }
}

pub(crate) async fn migrate(
    database: ServiceDatabase,
    migrations: &'static [SchemaMigration],
    migrated: watch::Sender<bool>,
    cancel: CancellationToken,
) {
    let applied = until_success("schema migration", &cancel, || async {
        database.verify_schema().await?;
        database.migrate(migrations).await
    })
    .await;
    if applied.is_some() {
        tracing::info!(
            schema_version = SchemaMigration::latest(migrations),
            "schema migrated"
        );
        migrated.send_replace(true);
    }
}

pub(crate) async fn migrate_sqlite(
    database: panel_sqlite::ServiceDatabase,
    migrations: &'static [panel_sqlite::SchemaMigration],
    migrated: watch::Sender<bool>,
    cancel: CancellationToken,
) {
    let applied = until_success("schema migration", &cancel, || async {
        database.migrate(migrations).await
    })
    .await;
    if applied.is_some() {
        tracing::info!(
            schema_version = panel_sqlite::SchemaMigration::latest(migrations),
            "schema migrated"
        );
        migrated.send_replace(true);
    }
}

pub(crate) async fn register(
    context: Context,
    settings: Arc<JetStreamSettings>,
    descriptor: ServiceDescriptor,
    ttl: Duration,
    policy: RegistrationPolicy,
    cancel: CancellationToken,
) {
    let Some(registry) = until_success("broker provisioning", &cancel, || async {
        ensure_streams(&context, &settings).await?;
        JetStreamServiceRegistry::provision(&context, &settings, ttl).await
    })
    .await
    else {
        return;
    };
    maintain_registration(Arc::new(registry), descriptor, policy, cancel.cancelled()).await;
}

/// Relays the outbox while this instance leads it; other instances stand by
/// and take over when the leader's session ends.
pub(crate) async fn relay(
    outbox: PgOutbox,
    publisher: JetStreamPublisher,
    options: RelayOptions,
    mut migrated: watch::Receiver<bool>,
    cancel: CancellationToken,
) {
    tokio::select! {
        () = cancel.cancelled() => return,
        ready = migrated.wait_for(|migrated| *migrated) => if ready.is_err() { return },
    }
    let publisher = Arc::new(publisher);
    loop {
        let Some(leadership) =
            until_success("outbox leadership", &cancel, || outbox.try_lead()).await
        else {
            return;
        };
        let Some(mut leadership) = leadership else {
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(STANDBY_INTERVAL) => continue,
            }
        };
        let Some(wakeup) = until_success("outbox listener", &cancel, || outbox.listen()).await
        else {
            let _ = leadership.release().await;
            return;
        };
        tracing::info!("leading the outbox relay");
        let purge = tokio::spawn(purge_published(outbox.clone(), cancel.child_token()));
        let relay = OutboxRelay::new(
            Arc::new(outbox.clone()),
            publisher.clone(),
            Arc::new(wakeup),
            options.clone(),
        );
        let lost = tokio::select! {
            () = relay.run(cancel.cancelled()) => false,
            () = leadership.lost(LEADERSHIP_CHECK_INTERVAL) => true,
        };
        purge.abort();
        if !lost {
            let _ = leadership.release().await;
            return;
        }
        tracing::warn!("outbox relay leadership lost; standing by");
    }
}

/// Relays a module's outbox. One process owns each SQLite file, so it
/// relays without leadership.
pub(crate) async fn relay_sqlite(
    outbox: SqliteOutbox,
    publisher: JetStreamPublisher,
    options: RelayOptions,
    mut migrated: watch::Receiver<bool>,
    cancel: CancellationToken,
) {
    tokio::select! {
        () = cancel.cancelled() => return,
        ready = migrated.wait_for(|migrated| *migrated) => if ready.is_err() { return },
    }
    let purge = tokio::spawn(purge_sqlite(outbox.clone(), cancel.child_token()));
    let relay = OutboxRelay::new(
        Arc::new(outbox.clone()),
        Arc::new(publisher),
        Arc::new(outbox.wakeup()),
        options,
    );
    relay.run(cancel.cancelled()).await;
    purge.abort();
}

async fn purge_sqlite(outbox: SqliteOutbox, cancel: CancellationToken) {
    let mut ticker = tokio::time::interval(PURGE_INTERVAL);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => {}
        }
        match outbox
            .purge_published(PUBLISHED_RETENTION, PURGE_BATCH)
            .await
        {
            Ok(0) => {}
            Ok(purged) => tracing::debug!(purged, "purged published outbox rows"),
            Err(error) => tracing::warn!(error_code = %error.code, "outbox purge failed"),
        }
    }
}

async fn purge_published(outbox: PgOutbox, cancel: CancellationToken) {
    let mut ticker = tokio::time::interval(PURGE_INTERVAL);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => {}
        }
        match outbox
            .purge_published(PUBLISHED_RETENTION, PURGE_BATCH)
            .await
        {
            Ok(0) => {}
            Ok(purged) => tracing::debug!(purged, "purged published outbox rows"),
            Err(error) => tracing::warn!(error_code = %error.code, "outbox purge failed"),
        }
    }
}
