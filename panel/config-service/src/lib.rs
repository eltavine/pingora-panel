#![forbid(unsafe_code)]

//! Composition of `config-service`, the write model of gateway
//! configuration and the only caller of the gateway publication protocol.
//!
//! The publication API compiles documents, prepares and activates them on
//! the gateway, and records each activation's receipt in the service schema
//! so that a retried activation replays its receipt instead of running
//! again. A reconciler completes activations interrupted by a crash,
//! restores the desired configuration to a gateway that lost it, and
//! suspends publication while the gateway runs an unknown configuration.

mod deployments;
mod publication;
mod receipts;
mod reconcile;
mod recording;

pub use deployments::{PendingActivation, PgDeployments, PreparedRecord};
pub use publication::PublicationService;
pub use receipts::PgActivationReceipts;
pub use reconcile::{Reconciler, Reconciliation, ReconciliationCheck, ReconciliationWatch};
pub use recording::RecordingUseCases;

use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use panel_application::{GatewayService, GatewayUseCases, IdempotentGatewayUseCases};
use panel_config_json::{JsonCompilerConfig, JsonRuntimeSnapshotCompiler};
use panel_contracts::{config::v1::publication_server::PublicationServer, CONFIG_V1};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_health::Impact;
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_postgres::{SchemaMigration, SqlIdentifier};
use panel_service::Environment;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub const SERVICE: &str = "config-service";
pub const SCHEMA: &str = "config";
pub const GATEWAY_URL_ENV: &str = "PINGORA_PANEL_GATEWAY_URL";
/// How often the gateway is reconciled after the startup reconciliation.
pub const RECONCILE_INTERVAL_MS_ENV: &str = "PINGORA_PANEL_RECONCILE_INTERVAL_MS";
const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:50051";
/// Largest publication message, sized for a maximal JSON document.
const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

pub const MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration::new(
        10_000,
        "activation receipts",
        include_str!("../migrations/10000_activation_receipts.sql"),
    ),
    SchemaMigration::new(
        10_001,
        "deployments",
        include_str!("../migrations/10001_deployments.sql"),
    ),
];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9181)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50061)),
    }
}

/// The process; while the gateway is unreachable the service stays
/// readable but suspends publication.
pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let gateway_url = env
        .string(GATEWAY_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.into());
    let process = ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let gateway = match process.peer_channel(&gateway_url, ServiceName::new("gatewayd")?)? {
        Some(channel) => GatewayGrpcClient::from_channel_with_config(
            channel,
            GatewayGrpcClientConfig::default(),
        )?,
        None => GatewayGrpcClient::connect_lazy(gateway_url, GatewayGrpcClientConfig::default())?,
    };
    let gateway_health = gateway.health_check();
    let reconcile_interval = env.millis(RECONCILE_INTERVAL_MS_ENV, DEFAULT_RECONCILE_INTERVAL)?;
    let gateway: Arc<dyn GatewayUseCases> = Arc::new(GatewayService::new(
        Arc::new(gateway),
        Arc::new(JsonRuntimeSnapshotCompiler::new(
            JsonCompilerConfig::default(),
        )?),
    ));
    let receipts = Arc::new(PgActivationReceipts::new(process.database()));
    let deployments = PgDeployments::new(process.database());
    let (reconciler, reconciliation) = Reconciler::new(
        Arc::clone(&gateway),
        Arc::clone(&receipts),
        deployments.clone(),
    );
    let use_cases: Arc<dyn GatewayUseCases> = Arc::new(RecordingUseCases::new(
        Arc::new(IdempotentGatewayUseCases::new(gateway, receipts)),
        deployments,
        reconciliation.clone(),
    ));
    Ok(process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(CONFIG_V1))
        .with_capability(Capability::new("config.publication", "1")?)
        .with_check(Arc::new(gateway_health), Impact::Degrading)
        .with_peer_access(
            panel_contracts::config::v1::publication_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_check(
            Arc::new(ReconciliationCheck(reconciliation)),
            Impact::Degrading,
        )
        .on_start(move |running| {
            running.spawn(reconciler.run(reconcile_interval, running.shutdown_token()));
            Ok(())
        })
        .with_grpc_service(
            PublicationServer::from_arc(Arc::new(PublicationService::new(use_cases)))
                .max_decoding_message_size(MAX_MESSAGE_BYTES)
                .max_encoding_message_size(MAX_MESSAGE_BYTES),
        ))
}
