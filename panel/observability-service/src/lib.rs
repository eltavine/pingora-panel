#![forbid(unsafe_code)]

//! Composition of `observability-service`, the owner of alert rules,
//! silences and saved queries, which fronts the metrics and log backends.

mod alerts;
mod host;
mod logql;
mod logs;
mod loki;
mod promql;
mod traffic;

pub use alerts::{
    AlertChannels, AlertRules, AlertsService, Cause, ChannelKind, ChannelRecord, Comparison,
    Evaluator, Measure, Notices, NotificationRecord, Notifier, RuleRecord, RuleSpec, Severity,
    State, TestOutcome,
};
pub use host::HostService;
pub use logql::Filter;
pub use logs::LogsService;
pub use loki::Loki;
pub use promql::Scope;
pub use traffic::TrafficService;

use panel_contracts::{
    observability::v1::{alerts_server, host_server, logs_server, traffic_server},
    OBSERVABILITY_V1,
};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::{PanelError, Result};
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_postgres::{EventLog, SchemaMigration, SqlIdentifier};
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_service::Environment;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub const SERVICE: &str = "observability-service";
pub const SCHEMA: &str = "observability";

/// Where Prometheus, which keeps the gateway's metrics, answers queries.
pub const PROMETHEUS_URL_ENV: &str = "PINGORA_PANEL_PROMETHEUS_URL";
const DEFAULT_PROMETHEUS_URL: &str = "http://127.0.0.1:9090";
const PROMETHEUS_TIMEOUT: Duration = Duration::from_secs(10);
/// Where Loki, which keeps the gateway's logs, answers queries.
pub const LOKI_URL_ENV: &str = "PINGORA_PANEL_LOKI_URL";
const DEFAULT_LOKI_URL: &str = "http://127.0.0.1:3100";
/// The deployment's master keys, which seal alert channels' endpoints.
pub const MASTER_KEYS_ENV: &str = "PINGORA_PANEL_MASTER_KEYS";
/// Origins the console is reached at; notifications link to the first.
pub const PUBLIC_ORIGINS_ENV: &str = "PINGORA_PANEL_PUBLIC_ORIGINS";

pub const MIGRATIONS: &[SchemaMigration] = &[SchemaMigration::new(
    10_000,
    "alert rules, channels and notifications",
    include_str!("../migrations/10000_alerts.sql"),
)];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9183)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50063)),
    }
}

/// A client of the Prometheus HTTP API at `url`.
pub fn prometheus(url: &str) -> Result<prometheus_http_query::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .timeout(PROMETHEUS_TIMEOUT)
        .build()
        .map_err(|error| PanelError::internal(format!("cannot build an HTTP client: {error}")))?;
    prometheus_http_query::Client::from(client, url).map_err(|error| {
        PanelError::invalid_argument(format!("invalid {PROMETHEUS_URL_ENV}: {error}"))
    })
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let url = env
        .string(PROMETHEUS_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_PROMETHEUS_URL.to_owned());
    let traffic = TrafficService::new(prometheus(&url)?);
    let host = HostService::new(prometheus(&url)?);
    let loki = env
        .string(LOKI_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_LOKI_URL.to_owned());
    let logs = LogsService::new(Loki::new(&loki).map_err(|error| {
        PanelError::invalid_argument(format!("invalid {LOKI_URL_ENV}: {}", error.message))
    })?);
    let vault = env
        .secret(MASTER_KEYS_ENV)?
        .map(|keys| EnvelopeVault::from_keys(&keys))
        .transpose()?
        .map(|vault| Arc::new(vault) as Arc<dyn SecretVault>);
    if vault.is_none() {
        tracing::warn!("{MASTER_KEYS_ENV} is not set; alert channels cannot be kept");
    }
    let console = env.string(PUBLIC_ORIGINS_ENV)?.and_then(|origins| {
        origins
            .split(',')
            .map(str::trim)
            .find(|origin| !origin.is_empty())
            .map(str::to_owned)
    });
    let service = ServiceName::new(SERVICE)?;
    let process = ControlPlaneProcess::new(
        service.clone(),
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let events = EventLog::new(process.database(), service);
    let notices = Arc::new(Notices::new(console));
    let rules = AlertRules::new(process.database(), events.clone(), Arc::clone(&notices));
    let channels = AlertChannels::new(process.database(), events, vault);
    let notifier = Notifier::new(process.database(), channels.clone(), notices)?;
    let evaluator = Evaluator::new(rules.clone(), prometheus(&url)?, SERVICE)?;
    let alerts = AlertsService::new(rules, channels, notifier.clone());
    Ok(process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(OBSERVABILITY_V1))
        .with_capability(Capability::new("observability.traffic", "1")?)
        .with_capability(Capability::new("observability.logs", "1")?)
        .with_capability(Capability::new("observability.alerts", "1")?)
        .with_capability(Capability::new("observability.host", "1")?)
        .with_peer_access(host_server::SERVICE_NAME, [ServiceName::new("panel-api")?])
        .with_peer_access(
            traffic_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_peer_access(logs_server::SERVICE_NAME, [ServiceName::new("panel-api")?])
        .with_peer_access(
            alerts_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_grpc_service(traffic_server::TrafficServer::new(traffic))
        .with_grpc_service(logs_server::LogsServer::new(logs))
        .with_grpc_service(alerts_server::AlertsServer::new(alerts))
        .with_grpc_service(host_server::HostServer::new(host))
        .on_start(move |running| {
            running.spawn(evaluator.run(running.migrated(), running.shutdown_token()));
            running.spawn(notifier.run(running.migrated(), running.shutdown_token()));
            Ok(())
        }))
}
