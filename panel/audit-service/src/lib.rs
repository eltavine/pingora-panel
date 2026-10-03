#![forbid(unsafe_code)]

//! Composition of `audit-service`, the owner of the audit trail: every
//! domain event of every service, appended once to a hash chain, and the
//! queries that read and verify it.

mod chain;
mod query;
mod store;

pub use chain::Entry;
pub use store::{Filter, PgAuditStore, Record, Verification};

use async_trait::async_trait;
use panel_contracts::{audit::v1::audit_query_server, AUDIT_V1};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_events::{ConsumerName, EventDelivery, EventHandler, HandlerOutcome};
use panel_jetstream::{ConsumerSpec, JetStreamConsumer, JetStreamSettings};
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_postgres::{SchemaMigration, SqlIdentifier};
use panel_service::Environment;
use query::AuditQueryService;
use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub const SERVICE: &str = "audit-service";
pub const SCHEMA: &str = "audit";
/// The durable consumer that reads every event type.
pub const CONSUMER: &str = "audit";
const RETRY_AFTER: Duration = Duration::from_secs(2);
const RECONNECT_AFTER: Duration = Duration::from_secs(5);

pub const MIGRATIONS: &[SchemaMigration] = &[SchemaMigration::new(
    10_000,
    "audit records, chain head and checkpoints",
    include_str!("../migrations/10000_audit_records.sql"),
)];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9184)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50064)),
    }
}

/// Appends each delivered event to the audit trail.
pub struct AuditWriter {
    store: PgAuditStore,
}

impl AuditWriter {
    pub fn new(store: PgAuditStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl EventHandler for AuditWriter {
    async fn handle(&self, delivery: &EventDelivery) -> HandlerOutcome {
        HandlerOutcome::from_result(
            self.store.append(delivery.envelope()).await.map(|_| ()),
            RETRY_AFTER,
        )
    }
}

pub fn process(
    _env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let process = ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let store = PgAuditStore::new(process.database());
    let writer: Arc<dyn EventHandler> = Arc::new(AuditWriter::new(store.clone()));
    let spec = ConsumerSpec::new(ConsumerName::new(CONSUMER)?, vec![">".to_owned()])?;
    Ok(process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(AUDIT_V1))
        .with_capability(Capability::new("audit.query", "1")?)
        .with_peer_access(
            audit_query_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_grpc_service(audit_query_server::AuditQueryServer::new(
            AuditQueryService::new(store),
        ))
        .on_start(move |running| {
            running.spawn(consume(
                running.jetstream().clone(),
                Arc::clone(running.jetstream_settings()),
                spec,
                writer,
                running.migrated(),
                running.shutdown_token(),
            ));
            Ok(())
        }))
}

/// Feeds the audit writer once the schema exists, reconnecting while the
/// broker is unavailable.
async fn consume(
    context: async_nats::jetstream::Context,
    settings: Arc<JetStreamSettings>,
    spec: ConsumerSpec,
    writer: Arc<dyn EventHandler>,
    migrated: impl Future<Output = bool>,
    cancel: CancellationToken,
) {
    tokio::select! {
        () = cancel.cancelled() => return,
        ready = migrated => if !ready { return },
    }
    loop {
        match JetStreamConsumer::ensure(&context, Arc::clone(&settings), spec.clone()).await {
            Ok(consumer) => match consumer
                .run(Arc::clone(&writer), cancel.clone().cancelled_owned())
                .await
            {
                Ok(()) => return,
                Err(error) => tracing::warn!(error_code = %error.code, "audit consumer stopped"),
            },
            Err(error) => tracing::warn!(error_code = %error.code, "audit consumer unavailable"),
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(RECONNECT_AFTER) => {}
        }
    }
}
