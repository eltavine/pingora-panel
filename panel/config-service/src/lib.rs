#![forbid(unsafe_code)]

//! Composition of `config-service`, the write model of gateway
//! configuration and the only caller of the gateway publication protocol.
//!
//! The publication API compiles documents, prepares and activates them on
//! the gateway, and records each activation's receipt in the module's database
//! so that a retried activation replays its receipt instead of running
//! again. A reconciler completes activations interrupted by a crash,
//! restores the desired configuration to a gateway that lost it, and
//! suspends publication while the gateway runs an unknown configuration.

pub mod approval_rules;
mod approvals;
mod configuration;
mod deployments;
mod draft;
mod language;
mod lua_test;
#[cfg(test)]
mod memory;
mod operations;
mod publication;
mod receipts;
mod reconcile;
mod recording;
mod revisions;
mod scope;
pub mod store;
mod transport;

pub use approvals::SqliteApprovals;
pub use configuration::ConfigurationService;
pub use deployments::{PendingActivation, PreparedRecord, SqliteDeployments};
pub use draft::SqliteDrafts;
pub use publication::PublicationService;
pub use receipts::SqliteActivationReceipts;
pub use reconcile::{Reconciler, Reconciliation, ReconciliationCheck, ReconciliationWatch};
pub use recording::RecordingUseCases;
pub use revisions::SqliteRevisions;
pub use store::DraftState;
pub use transport::ConfigurationTransport;

use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use panel_application::{GatewayService, GatewayUseCases, IdempotentGatewayUseCases};
use panel_config_json::{JsonCompilerConfig, JsonRuntimeSnapshotCompiler};
use panel_contracts::{
    config::v1::{
        configuration_server::ConfigurationServer, publication_server::PublicationServer,
    },
    CONFIG_V1,
};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::{PanelError, Result};
use panel_health::Impact;
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_service::{loopback_channel, Environment};
use panel_sqlite::{EventLog, SchemaMigration};
use plugin_contracts::{plugin_of, PLUGIN_METADATA};
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub const SERVICE: &str = "config-service";
/// The module's SQLite file in the data directory, `config.db`.
pub const MODULE: &str = "config";
pub const GATEWAY_URL_ENV: &str = "PINGORA_PANEL_GATEWAY_URL";
/// The engine that runs the configuration: `gatewayd`, the default, or
/// `plugin:<name>` for the gateway engine a plugin provides (ADR 0044).
pub const GATEWAY_ENGINE_ENV: &str = "PINGORA_PANEL_GATEWAY_ENGINE";
/// `plugins-service`, which serves the gateway engines plugins provide.
pub const PLUGINS_URL_ENV: &str = "PINGORA_PANEL_PLUGINS_URL";
const DEFAULT_PLUGINS_URL: &str = "http://127.0.0.1:50066";
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
    SchemaMigration::new(
        10_002,
        "draft configuration",
        include_str!("../migrations/10002_draft_configuration.sql"),
    ),
    SchemaMigration::new(
        10_003,
        "configuration revisions",
        include_str!("../migrations/10003_configuration_revisions.sql"),
    ),
    SchemaMigration::new(
        10_004,
        "approval policies, requests and approvals",
        include_str!("../migrations/10004_approvals.sql"),
    ),
];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9181)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50061)),
    }
}

/// The plugin whose gateway engine runs the configuration, when
/// [`GATEWAY_ENGINE_ENV`] names one.
fn gateway_plugin(engine: Option<&str>) -> Result<Option<String>> {
    match engine {
        None | Some("gatewayd") => Ok(None),
        Some(value) => plugin_of(value)
            .map(|plugin| Some(plugin.to_owned()))
            .ok_or_else(|| {
                PanelError::invalid_argument(format!(
                    "{GATEWAY_ENGINE_ENV} is gatewayd or plugin:<name>, not {value:?}"
                ))
            }),
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
        MODULE,
    )?;
    let gateway = match gateway_plugin(env.string(GATEWAY_ENGINE_ENV)?.as_deref())? {
        Some(plugin) => {
            let plugins_url = env
                .string(PLUGINS_URL_ENV)?
                .unwrap_or_else(|| DEFAULT_PLUGINS_URL.into());
            let channel =
                match process.peer_channel(&plugins_url, ServiceName::new("plugins-service")?)? {
                    Some(channel) => channel,
                    None => loopback_channel(
                        "plugins service",
                        plugins_url,
                        Duration::from_secs(5),
                        Duration::from_secs(60),
                    )?,
                };
            GatewayGrpcClient::from_channel_with_config(
                channel,
                GatewayGrpcClientConfig::default(),
            )?
            .with_metadata(PLUGIN_METADATA, &plugin)?
        }
        None => match process.peer_channel(&gateway_url, ServiceName::new("gatewayd")?)? {
            Some(channel) => GatewayGrpcClient::from_channel_with_config(
                channel,
                GatewayGrpcClientConfig::default(),
            )?,
            None => {
                GatewayGrpcClient::connect_lazy(gateway_url, GatewayGrpcClientConfig::default())?
            }
        },
    };
    let gateway_health = gateway.health_check();
    let reconcile_interval = env.millis(RECONCILE_INTERVAL_MS_ENV, DEFAULT_RECONCILE_INTERVAL)?;
    let gateway: Arc<dyn GatewayUseCases> = Arc::new(GatewayService::new(
        Arc::new(gateway),
        Arc::new(JsonRuntimeSnapshotCompiler::new(
            JsonCompilerConfig::default(),
        )?),
    ));
    let receipts = Arc::new(SqliteActivationReceipts::new(process.database()));
    let events = EventLog::new(process.database(), ServiceName::new(SERVICE)?);
    let deployments = SqliteDeployments::new(process.database());
    let (reconciler, reconciliation) = Reconciler::new(
        Arc::clone(&gateway),
        Arc::clone(&receipts),
        deployments.clone(),
    );
    let use_cases: Arc<dyn GatewayUseCases> = Arc::new(RecordingUseCases::new(
        Arc::new(IdempotentGatewayUseCases::new(gateway, receipts)),
        deployments,
        reconciliation.clone(),
        events.clone(),
    ));
    let drafts = Arc::new(SqliteDrafts::new(process.database(), events.clone()));
    let revisions = Arc::new(SqliteRevisions::new(process.database()));
    let approvals = Arc::new(SqliteApprovals::new(process.database(), events.clone()));
    Ok(process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(CONFIG_V1))
        .with_capability(Capability::new("config.publication", "1")?)
        .with_capability(Capability::new("config.configuration", "1")?)
        .with_check(Arc::new(gateway_health), Impact::Degrading)
        .with_peer_access(
            panel_contracts::config::v1::publication_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_peer_access(
            panel_contracts::config::v1::configuration_server::SERVICE_NAME,
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
            ConfigurationServer::new(ConfigurationTransport::new(Arc::new(
                ConfigurationService::new(
                    drafts,
                    revisions,
                    approvals,
                    Arc::clone(&use_cases),
                    Arc::new(events),
                ),
            )))
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES),
        )
        .with_grpc_service(
            PublicationServer::from_arc(Arc::new(PublicationService::new(use_cases)))
                .max_decoding_message_size(MAX_MESSAGE_BYTES)
                .max_encoding_message_size(MAX_MESSAGE_BYTES),
        ))
}

#[async_trait::async_trait]
impl store::EventRecorder for EventLog {
    async fn record(
        &self,
        event: panel_events::EventDraft,
        scope: &panel_events::RequestScope,
        actor: &str,
    ) {
        self.record_draft(event, scope, actor).await;
    }
}
