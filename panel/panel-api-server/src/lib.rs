#![forbid(unsafe_code)]

//! Composition of `panel-api`, the single public management entry point.
//!
//! The process serves the REST API and the web console on its public
//! listener. Publication is delegated to `config-service`; the service
//! directory is read from the broker. While a dependency the API needs for
//! changes is down the process runs degraded: reads continue and changes
//! are refused with a retryable 503.

mod console;
mod directory;
mod listener;
mod operations;
mod roles;

use listener::PublicListener;
pub use listener::UNIX_PREFIX;

use audit_grpc_client::AuditClient;
use automation_grpc_client::AutomationClient;
use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use identity_oidc::OidcClient;
use identity_postgres::PgIdentityStore;
use observability_grpc_client::ObservabilityClient;
use panel_api::{router_with_config, AccessSettings, ApiConfig, ApiState};
use panel_application::RecordedRuntime;
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::{PanelError, Result};
use panel_health::Impact;
use panel_identity::{
    Identity, IdentitySettings, OpenIdConnect, ProviderDirectory, ProviderSignIns, SecretHash,
    SessionPolicy, WorkloadIdentity,
};
use panel_metrics::{HttpServerMetrics, RoutedRequest};
use panel_platform::ServiceName;
use panel_postgres::{EventLog, SqlIdentifier};
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_service::{measured, Environment};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tls_probe_rustls::RustlsProbe;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

pub const SERVICE: &str = "panel-api";
pub const SCHEMA: &str = "identity";
/// The public listener: a loopback `ip:port`, or `unix:/path` for a Unix
/// domain socket that a local reverse proxy in the socket's group reaches.
pub const HTTP_ADDRESS_ENV: &str = "PINGORA_PANEL_HTTP_ADDR";
pub const CONFIG_URL_ENV: &str = "PINGORA_PANEL_CONFIG_URL";
/// `audit-service`, which serves the audit trail.
pub const AUDIT_URL_ENV: &str = "PINGORA_PANEL_AUDIT_URL";
/// `observability-service`, which reads what the gateway served.
pub const OBSERVABILITY_URL_ENV: &str = "PINGORA_PANEL_OBSERVABILITY_URL";
/// `automation-service`, which keeps the certificate inventory.
pub const AUTOMATION_URL_ENV: &str = "PINGORA_PANEL_AUTOMATION_URL";
/// The gateway's runtime API, for data plane operations and upstream health.
pub const GATEWAY_URL_ENV: &str = "PINGORA_PANEL_GATEWAY_URL";
/// Directory holding the built web console; the API is served without it.
pub const WEB_ROOT_ENV: &str = "PINGORA_PANEL_WEB_ROOT";
/// The one-time token that creates the first account; `_FILE` names a file
/// holding it.
pub const BOOTSTRAP_TOKEN_ENV: &str = "PINGORA_PANEL_BOOTSTRAP_TOKEN";
/// At least 16 bytes keying every password hash; `_FILE` names a file
/// holding it. Changing it invalidates every password.
pub const PASSWORD_PEPPER_ENV: &str = "PINGORA_PANEL_PASSWORD_PEPPER";
/// Milliseconds without activity after which a session ends.
pub const SESSION_IDLE_ENV: &str = "PINGORA_PANEL_SESSION_IDLE_MS";
/// Milliseconds after login after which a session ends.
pub const SESSION_LIFETIME_ENV: &str = "PINGORA_PANEL_SESSION_LIFETIME_MS";
/// Comma-separated origins the console is reached at, such as
/// `https://panel.example`, checked against unsafe browser requests that
/// carry no `Sec-Fetch-Site`.
pub const PUBLIC_ORIGINS_ENV: &str = "PINGORA_PANEL_PUBLIC_ORIGINS";
/// Master keys that seal identity provider secrets and refresh tokens, one
/// base64-encoded 256-bit key per line; the first seals new values. Usually
/// given as `_FILE`.
pub const MASTER_KEYS_ENV: &str = "PINGORA_PANEL_MASTER_KEYS";
/// How long one request to an identity provider may take.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);
/// How often sessions due to be rechecked with their provider are looked for.
const RECHECK_SWEEP: Duration = Duration::from_secs(60);

const DEFAULT_HTTP_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);
const DEFAULT_CONFIG_URL: &str = "http://127.0.0.1:50061";
const DEFAULT_AUDIT_URL: &str = "http://127.0.0.1:50064";
const DEFAULT_OBSERVABILITY_URL: &str = "http://127.0.0.1:50063";
const DEFAULT_AUTOMATION_URL: &str = "http://127.0.0.1:50062";
const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:50051";
const DEFAULT_WEB_ROOT: &str = "/usr/share/pingora-panel/web";

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9180)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50060)),
    }
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let http_address = env
        .string(HTTP_ADDRESS_ENV)?
        .unwrap_or_else(|| DEFAULT_HTTP_ADDRESS.to_string());
    let config_url = env
        .string(CONFIG_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_CONFIG_URL.into());
    let gateway_url = env
        .string(GATEWAY_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.into());
    let audit_url = env
        .string(AUDIT_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_AUDIT_URL.into());
    let automation_url = env
        .string(AUTOMATION_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_AUTOMATION_URL.into());
    let observability_url = env
        .string(OBSERVABILITY_URL_ENV)?
        .unwrap_or_else(|| DEFAULT_OBSERVABILITY_URL.into());
    let web_root = PathBuf::from(
        env.string(WEB_ROOT_ENV)?
            .unwrap_or_else(|| DEFAULT_WEB_ROOT.into()),
    );
    let bootstrap = env.secret(BOOTSTRAP_TOKEN_ENV)?;
    let pepper = env.secret(PASSWORD_PEPPER_ENV)?;
    if pepper.as_ref().is_some_and(|pepper| pepper.len() < 16) {
        return Err(PanelError::invalid_argument(format!(
            "{PASSWORD_PEPPER_ENV} must have at least 16 bytes"
        )));
    }
    let defaults = SessionPolicy::default();
    let sessions = SessionPolicy {
        idle: env.millis(SESSION_IDLE_ENV, defaults.idle)?,
        absolute: env.millis(SESSION_LIFETIME_ENV, defaults.absolute)?,
        ..defaults
    };
    let origins: Vec<String> = env
        .string(PUBLIC_ORIGINS_ENV)?
        .map(|origins| {
            origins
                .split(',')
                .map(|origin| origin.trim().trim_end_matches('/').to_owned())
                .filter(|origin| !origin.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let vault = env
        .secret(MASTER_KEYS_ENV)?
        .map(|keys| EnvelopeVault::from_keys(&keys))
        .transpose()?;
    // Bound now so a taken address fails the start before anything else runs.
    let listener = PublicListener::bind(HTTP_ADDRESS_ENV, &http_address)?;
    let mut process = ControlPlaneProcess::new(
        ServiceName::new(SERVICE)?,
        env!("CARGO_PKG_VERSION"),
        settings,
        SqlIdentifier::new(SCHEMA)?,
    )?;
    let api_metrics = HttpServerMetrics::<RoutedRequest>::register(process.metrics().registry());
    let config = match process.peer_channel(&config_url, ServiceName::new("config-service")?)? {
        Some(channel) => {
            ConfigPublicationClient::from_channel(channel, ConfigClientConfig::default())
        }
        None => ConfigPublicationClient::connect_lazy(config_url, ConfigClientConfig::default())?,
    };
    let gateway = match process.peer_channel(&gateway_url, ServiceName::new("gatewayd")?)? {
        Some(channel) => GatewayGrpcClient::from_channel_with_config(
            channel,
            GatewayGrpcClientConfig::default(),
        )?,
        None => GatewayGrpcClient::connect_lazy(gateway_url, GatewayGrpcClientConfig::default())?,
    };
    let audit = match process.peer_channel(&audit_url, ServiceName::new("audit-service")?)? {
        Some(channel) => AuditClient::from_channel(channel),
        None => AuditClient::connect_lazy(audit_url)?,
    };
    let automation =
        match process.peer_channel(&automation_url, ServiceName::new("automation-service")?)? {
            Some(channel) => AutomationClient::from_channel(channel),
            None => AutomationClient::connect_lazy(automation_url)?,
        };
    let observability = match process.peer_channel(
        &observability_url,
        ServiceName::new("observability-service")?,
    )? {
        Some(channel) => ObservabilityClient::from_channel(channel),
        None => ObservabilityClient::connect_lazy(observability_url)?,
    };
    let audit_health = audit.health_check();
    let observability_health = observability.health_check();
    let automation_health = automation.health_check();
    let config_health = config.health_check();
    let events = EventLog::new(process.database(), ServiceName::new(SERVICE)?);
    let operations = Arc::new(operations::OutboxOperations(events.clone()));
    let runtime = RecordedRuntime::new(Arc::new(gateway), operations.clone());
    let store = Arc::new(PgIdentityStore::new(process.database(), events));
    let roles = roles::BuiltInRoles::new(Arc::clone(&store), bootstrap.is_some());
    let oidc = Arc::new(OidcClient::new(PROVIDER_TIMEOUT)?);
    let workloads = WorkloadIdentity::new(store.clone(), store.clone(), oidc.clone());
    let providers = identity_providers(&store, oidc, vault, origins.first().cloned(), sessions);
    let identity = Identity::new(
        store,
        IdentitySettings {
            sessions,
            pepper: pepper.map(String::into_bytes),
            bootstrap: bootstrap.as_deref().map(SecretHash::of),
            ..IdentitySettings::default()
        },
    );
    let access = AccessSettings {
        origins,
        ..AccessSettings::default()
    };
    Ok(process
        .with_migrations(identity_postgres::MIGRATIONS)
        .with_database_impact(Impact::Degrading)
        .with_check(Arc::new(roles), Impact::Required)
        .with_check(Arc::new(config_health), Impact::Degrading)
        .with_check(Arc::new(audit_health), Impact::Informational)
        .with_check(Arc::new(automation_health), Impact::Informational)
        .with_check(Arc::new(observability_health), Impact::Informational)
        .on_start(move |running| {
            let config = Arc::new(config);
            let state = ApiState::new(Arc::clone(&config))
                .with_configuration(config)
                .with_runtime(Arc::new(runtime))
                .with_audit(Arc::new(audit))
                .with_certificates(Arc::new(automation))
                .with_traffic(Arc::new(observability))
                .with_tls_probe(Arc::new(RustlsProbe::default()))
                .with_identity(identity, access)
                .with_access_audit(operations)
                .with_health(running.health())
                .with_directory(Arc::new(directory::RegistryDirectory::new(
                    running.jetstream().clone(),
                    Arc::clone(running.jetstream_settings()),
                )));
            if let Some(sign_ins) = providers
                .as_ref()
                .and_then(|(_, sign_ins)| sign_ins.clone())
            {
                running.spawn(recheck_sessions(sign_ins, running.shutdown_token()));
            }
            let state = state.with_workload_identity(workloads);
            let state = match providers {
                Some((directory, sign_ins)) => state.with_identity_providers(directory, sign_ins),
                None => state,
            };
            let api = measured(router_with_config(state, ApiConfig::default()), api_metrics);
            let app = console::with_console(api, &web_root)?;
            let shutdown = running.shutdown_token();
            running.spawn(async move {
                if let Err(error) = listener.serve(app, shutdown).await {
                    tracing::error!(%error, "public listener failed");
                }
            });
            Ok(())
        }))
}

/// Identity providers need master keys to seal their secrets, and sign-ins
/// through them a public origin for people to return to.
fn identity_providers(
    store: &Arc<PgIdentityStore>,
    connect: Arc<dyn OpenIdConnect>,
    vault: Option<EnvelopeVault>,
    public_origin: Option<String>,
    sessions: SessionPolicy,
) -> Option<(ProviderDirectory, Option<ProviderSignIns>)> {
    let Some(vault) = vault else {
        tracing::warn!("{MASTER_KEYS_ENV} is not set; identity providers are not available");
        return None;
    };
    let vault: Arc<dyn SecretVault> = Arc::new(vault);
    let directory = ProviderDirectory::new(
        store.clone(),
        store.clone(),
        Arc::clone(&vault),
        Arc::clone(&connect),
    );
    let Some(origin) = public_origin else {
        tracing::warn!(
            "{PUBLIC_ORIGINS_ENV} is not set; nobody can sign in through identity providers"
        );
        return Some((directory, None));
    };
    let sign_ins = ProviderSignIns::new(
        directory.clone(),
        store.clone(),
        store.clone(),
        connect,
        vault,
        origin,
        sessions,
    );
    Some((directory, Some(sign_ins)))
}

/// Asks providers about the sessions signed in through them, until shutdown.
async fn recheck_sessions(sign_ins: ProviderSignIns, shutdown: CancellationToken) {
    let mut sweep = tokio::time::interval(RECHECK_SWEEP);
    sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = sweep.tick() => match sign_ins.recheck().await {
                Ok(rechecked) if rechecked.ended + rechecked.unanswered > 0 => tracing::info!(
                    kept = rechecked.kept,
                    ended = rechecked.ended,
                    unanswered = rechecked.unanswered,
                    "rechecked sessions with their identity providers"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "sessions could not be rechecked"),
            },
        }
    }
}
