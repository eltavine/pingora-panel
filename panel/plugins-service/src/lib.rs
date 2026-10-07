#![forbid(unsafe_code)]

//! The plugins module of the control plane (ADR 0044). It finds signed
//! plugin packages in the plugins directory, runs the enabled ones as child
//! processes under their limits, serves their ports to the modules that use
//! them, and keeps what administrators decided about each: grants,
//! settings, limits and versions, the trusted publisher keys and the
//! secrets settings name.

mod service;
mod store;
mod transport;

pub use service::{Paths, PluginService};
pub use store::{PluginRecord, Store};
pub use transport::PluginsTransport;

use panel_contracts::{plugins::v1::plugins_server, PLUGINS_V1};
use panel_control_runtime::{ControlPlaneProcess, DefaultAddresses, ProcessSettings};
use panel_errors::Result;
use panel_platform::{Capability, ServiceName};
use panel_platform_codec::protocol_range;
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_service::Environment;
use panel_sqlite::{EventLog, SchemaMigration};
use plugin_host::{
    proxy::{
        Backups, Containers, Dns01, GatewayEngine, GatewayRuntime, Notifications, Port, PortProxy,
    },
    runtime::Runtime,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

pub const SERVICE: &str = "plugins-service";
pub const MODULE: &str = "plugins";
/// Master keys that seal the secrets plugins' settings name.
pub const MASTER_KEYS_ENV: &str = "PINGORA_PANEL_MASTER_KEYS";
/// The plugins directory of `<name>/<version>/` packages; `plugins` in the
/// data directory by default.
pub const PLUGINS_DIR_ENV: &str = "PINGORA_PANEL_PLUGINS_DIR";
/// Where running plugins' sockets go, which Unix socket paths keep short;
/// `plugins-run` in the data directory by default.
pub const PLUGINS_RUNTIME_DIR_ENV: &str = "PINGORA_PANEL_PLUGINS_RUNTIME_DIR";
/// How often enabled plugins that do not run are started again.
const RESTART_INTERVAL: Duration = Duration::from_secs(30);

pub const MIGRATIONS: &[SchemaMigration] = &[SchemaMigration::new(
    10_000,
    "plugins, trusted keys and secrets",
    include_str!("../migrations/10000_plugins.sql"),
)];

pub fn default_addresses() -> DefaultAddresses {
    DefaultAddresses {
        ops: SocketAddr::from(([127, 0, 0, 1], 9186)),
        grpc: SocketAddr::from(([127, 0, 0, 1], 50066)),
    }
}

/// Serves `P` through the plugins that provide it, to `callers`.
fn port<P: Port>(
    process: ControlPlaneProcess,
    runtime: &Arc<Runtime>,
    callers: &[&str],
) -> Result<ControlPlaneProcess> {
    let callers = callers
        .iter()
        .map(|caller| ServiceName::new(*caller))
        .collect::<Result<Vec<_>>>()?;
    Ok(process
        .with_peer_access(P::SERVICE, callers)
        .with_grpc_service(PortProxy::<P>::new(Arc::clone(runtime))))
}

pub fn process(
    env: &mut Environment<'_>,
    settings: ProcessSettings,
) -> Result<ControlPlaneProcess> {
    let vault = env
        .secret(MASTER_KEYS_ENV)?
        .map(|keys| EnvelopeVault::from_keys(&keys))
        .transpose()?
        .map(|vault| Arc::new(vault) as Arc<dyn SecretVault>);
    if vault.is_none() {
        tracing::warn!("{MASTER_KEYS_ENV} is not set; secrets for plugins cannot be kept");
    }
    let data = settings.data_directory().to_owned();
    let paths = Paths {
        packages: env
            .string(PLUGINS_DIR_ENV)?
            .map_or_else(|| data.join("plugins"), PathBuf::from),
        data: data.join("plugin-data"),
        sockets: env
            .string(PLUGINS_RUNTIME_DIR_ENV)?
            .map_or_else(|| data.join("plugins-run"), PathBuf::from),
    };
    let service = ServiceName::new(SERVICE)?;
    let process =
        ControlPlaneProcess::new(service.clone(), env!("CARGO_PKG_VERSION"), settings, MODULE)?;
    let (runtime, changes) = Runtime::new();
    let plugins = PluginService::new(
        Store::new(process.database()),
        EventLog::new(process.database(), service),
        Arc::clone(&runtime),
        paths,
        vault,
    );
    let process = process
        .with_migrations(MIGRATIONS)
        .with_protocol(protocol_range(PLUGINS_V1))
        .with_capability(Capability::new("plugins", "1")?)
        .with_peer_access(
            plugins_server::SERVICE_NAME,
            [ServiceName::new("panel-api")?],
        )
        .with_grpc_service(plugins_server::PluginsServer::new(PluginsTransport::new(
            Arc::new(plugins.clone()),
        )));
    let process = port::<Dns01>(process, &runtime, &["automation-service"])?;
    let process = port::<Backups>(process, &runtime, &["automation-service"])?;
    let process = port::<Notifications>(process, &runtime, &["observability-service"])?;
    let process = port::<Containers>(process, &runtime, &["panel-api"])?;
    let process = port::<GatewayEngine>(process, &runtime, &["config-service", "panel-api"])?;
    let process = port::<GatewayRuntime>(process, &runtime, &["config-service", "panel-api"])?;
    Ok(process.on_start(move |running| {
        let shutdown = running.shutdown_token();
        let migrated = running.migrated();
        running.spawn({
            let plugins = plugins.clone();
            let shutdown = running.shutdown_token();
            async move {
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = plugins.record_changes(changes) => {}
                }
            }
        });
        running.spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => return,
                ready = migrated => if !ready { return },
            }
            plugins.start().await;
            let mut restarts = tokio::time::interval(RESTART_INTERVAL);
            restarts.tick().await;
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = restarts.tick() => plugins.start_enabled().await,
                }
            }
            runtime.stop_all().await;
        });
        Ok(())
    }))
}
