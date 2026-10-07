//! The plugins behind the plugins port: the versions found in the plugins
//! directory, what administrators decided about each, and the processes
//! that run the enabled ones (ADR 0044).
//!
//! A change of a plugin starts the process it asks for before it is saved:
//! when the process does not start, nothing is saved and what ran keeps
//! running; when saving fails, what ran before is started again.

use crate::store::{PluginRecord, Store};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_application::{CommandContext, RequestScope};
use panel_errors::{PanelError, Result};
use panel_event_contracts::plugins::v1 as event;
use panel_events::{Actor, Principal, RequestId};
use panel_plugin_api::{
    HealthStatus, NewTrustedKey, PluginChange, PluginCommand, PluginHealth, PluginLimits,
    PluginList, PluginOutput, PluginQuery, PluginState, PluginView, PluginsPort, Secret,
    SecretView, TrustedKeyView, VersionView,
};
use panel_secrets::SecretVault;
use panel_sqlite::{storage_error, EventLog, SqliteOutbox};
use plugin_contracts::{
    v1::{secret_provider_client::SecretProviderClient, Manifest, ResolveRequest},
    PORTS, SECRETS_CAPABILITY,
};
use plugin_host::{
    catalog::{self, Found},
    limits::{self, Limits},
    manifest::{self, HOST_PROTOCOL_VERSIONS},
    process::Launch,
    runtime::{Change, Health, Runtime, State},
    settings::{self, Reference},
    signature::{self, TrustedKey},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::SystemTime,
};
use tokio::sync::mpsc;

const PLUGIN: &str = "plugin";
const KEY: &str = "plugin_key";
const SECRET: &str = "plugin_secret";
const CATALOG: &str = "catalog";
const SECRETS_PORT: &str = "secrets";

/// Where plugins and what they need live on disk.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The packages, as `<name>/<version>/`.
    pub packages: PathBuf,
    /// Each plugin's own directory goes in here, by name.
    pub data: PathBuf,
    /// The socket directories of running plugins; short enough for Unix
    /// socket paths.
    pub sockets: PathBuf,
}

#[derive(Default)]
struct Catalog {
    found: Vec<Found>,
    discovered_at: Option<DateTime<Utc>>,
}

impl Catalog {
    fn versions<'c>(&'c self, name: &'c str) -> impl Iterator<Item = &'c Found> + 'c {
        self.found.iter().filter(move |found| found.name == name)
    }

    fn version(&self, name: &str, version: &str) -> Option<&Found> {
        self.found
            .iter()
            .find(|found| found.name == name && found.version == version)
    }

    fn newest_valid(&self, name: &str) -> Option<&Found> {
        self.found
            .iter()
            .rev()
            .find(|found| found.name == name && found.is_valid())
    }

    /// The version a plugin's settings and limits are checked against: the
    /// one it runs, or else the newest that could.
    fn target(&self, record: &PluginRecord) -> Option<&Found> {
        record
            .active_version
            .as_deref()
            .and_then(|version| self.version(&record.name, version))
            .or_else(|| self.newest_valid(&record.name))
    }
}

#[derive(Clone, Copy)]
struct Cause<'a> {
    scope: &'a RequestScope,
    principal: &'a Principal,
}

/// What the module does on its own, such as starting enabled plugins.
fn system() -> (RequestScope, Principal) {
    let request = RequestId::new(format!("plugins-{}", uuid::Uuid::new_v4()))
        .expect("generated request IDs are valid");
    (
        RequestScope::new(request),
        Principal::system(Actor::new(crate::SERVICE).expect("the service name is an actor")),
    )
}

/// The event a change of a plugin writes.
enum Recorded {
    Granted(event::PluginGranted),
    Configured(event::PluginConfigured),
    Limited(event::PluginLimited),
    Enabled(event::PluginEnabled),
    Disabled(event::PluginDisabled),
    Upgraded(event::PluginUpgraded),
    RolledBack(event::PluginRolledBack),
}

fn output(value: &impl Serialize, etag: Option<String>) -> PluginOutput {
    PluginOutput {
        content: serde_json::to_vec(value).expect("API values serialize"),
        etag,
    }
}

fn nothing() -> PluginOutput {
    PluginOutput {
        content: Vec::new(),
        etag: None,
    }
}

fn host_limits(limits: PluginLimits) -> Limits {
    Limits {
        memory_bytes: limits.memory_bytes,
        cpu_seconds: limits.cpu_seconds,
        open_files: limits.open_files,
        concurrency: limits.concurrency,
        call_timeout_ms: limits.call_timeout_ms,
    }
}

fn api_limits(limits: Limits) -> PluginLimits {
    PluginLimits {
        memory_bytes: limits.memory_bytes,
        cpu_seconds: limits.cpu_seconds,
        open_files: limits.open_files,
        concurrency: limits.concurrency,
        call_timeout_ms: limits.call_timeout_ms,
    }
}

fn time(at: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(at)
}

fn health_view(version: &str, health: Health) -> PluginHealth {
    let mut view = PluginHealth::default();
    view.status = match health.state {
        State::Running => HealthStatus::Serving,
        _ => HealthStatus::Degraded,
    };
    view.version = version.to_owned();
    view.started_at = time(health.started_at);
    view.checked_at = health.checked_at.map(time);
    view.failures = health.failures;
    view.restarts = health.restarts;
    view.error = health.error;
    view
}

fn version_view(found: &Found) -> VersionView {
    let manifest = found.manifest.clone().unwrap_or_default();
    let resources = manifest.resources.unwrap_or_default();
    let mut view = VersionView::default();
    view.version = found.version.clone();
    view.problems = found.problems.clone();
    if found.manifest.is_none() && view.problems.is_empty() {
        view.problems.push("the manifest cannot be read".into());
    }
    view.compatible = manifest
        .protocol_versions
        .iter()
        .any(|version| HOST_PROTOCOL_VERSIONS.contains(version));
    view.config_schema = settings::schema(&manifest.config_schema).ok().flatten();
    view.resources = PluginLimits {
        memory_bytes: resources.memory_bytes,
        cpu_seconds: resources.cpu_seconds,
        open_files: resources.open_files,
        concurrency: resources.concurrency,
        call_timeout_ms: manifest.call_timeout_ms,
    };
    view.signed_by = found.signed_by.clone();
    view.publisher = manifest.publisher;
    view.description = manifest.description;
    view.homepage = manifest.homepage;
    view.ports = manifest.ports;
    view.capabilities = manifest.capabilities;
    view.protocol_versions = manifest.protocol_versions;
    view.executable_sha256 = manifest.executable_sha256;
    view
}

fn runnable<'c>(catalog: &'c Catalog, name: &str, version: &str) -> Result<&'c Found> {
    let found = catalog.version(name, version).ok_or_else(|| {
        PanelError::not_found(format!(
            "plugin {name} {version} is not in the plugins directory"
        ))
    })?;
    if !found.is_valid() {
        return Err(PanelError::validation_failed(format!(
            "plugin {name} {version} cannot run: {}",
            found.problems.join("; ")
        )));
    }
    Ok(found)
}

fn manifest_of(found: &Found) -> &Manifest {
    found
        .manifest
        .as_ref()
        .expect("validated versions have manifests")
}

/// The secret a vault reference names, sealed for this owner.
fn owner(name: &str) -> String {
    format!("plugin-secret/{name}")
}

/// Whether `value` names `vault:<name>` anywhere.
fn names_secret(value: &Value, reference: &str) -> bool {
    match value {
        Value::String(text) => text == reference,
        Value::Array(items) => items.iter().any(|item| names_secret(item, reference)),
        Value::Object(fields) => fields.values().any(|item| names_secret(item, reference)),
        _ => false,
    }
}

struct Inner {
    store: Store,
    events: EventLog,
    runtime: Arc<Runtime>,
    paths: Paths,
    vault: Option<Arc<dyn SecretVault>>,
    catalog: RwLock<Catalog>,
    /// Why an enabled plugin does not run, from its last start.
    failures: Mutex<HashMap<String, String>>,
    /// Changes are made one at a time.
    changes: tokio::sync::Mutex<()>,
}

/// The plugins module's port.
#[derive(Clone)]
pub struct PluginService {
    inner: Arc<Inner>,
}

impl PluginService {
    pub fn new(
        store: Store,
        events: EventLog,
        runtime: Arc<Runtime>,
        paths: Paths,
        vault: Option<Arc<dyn SecretVault>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                events,
                runtime,
                paths,
                vault,
                catalog: RwLock::new(Catalog::default()),
                failures: Mutex::new(HashMap::new()),
                changes: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.inner.runtime
    }

    fn catalog(&self) -> std::sync::RwLockReadGuard<'_, Catalog> {
        self.inner
            .catalog
            .read()
            .expect("the catalog is never poisoned")
    }

    fn failures(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.inner
            .failures
            .lock()
            .expect("failures are never poisoned")
    }

    fn vault(&self) -> Result<&dyn SecretVault> {
        self.inner.vault.as_deref().ok_or_else(|| {
            PanelError::unavailable("secrets cannot be kept until master keys are configured")
        })
    }

    /// Reads the plugins directory, then starts every enabled plugin.
    pub async fn start(&self) {
        let (scope, principal) = system();
        let cause = Cause {
            scope: &scope,
            principal: &principal,
        };
        {
            let _change = self.inner.changes.lock().await;
            if let Err(error) = self.discover(cause).await {
                tracing::warn!(%error, "the plugins directory cannot be read");
            }
        }
        self.start_enabled().await;
    }

    /// Starts the enabled plugins that do not run, in rounds while any
    /// starts, as the settings of one may name secrets another provides.
    pub async fn start_enabled(&self) {
        let _change = self.inner.changes.lock().await;
        let mut pending: Vec<PluginRecord> = match self.inner.store.plugins().await {
            Ok(records) => records
                .into_iter()
                .filter(|record| record.enabled && self.inner.runtime.get(&record.name).is_none())
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "plugins cannot be read");
                return;
            }
        };
        loop {
            let mut waiting = Vec::new();
            let before = pending.len();
            for record in pending {
                let started = match self.launch(&record).await {
                    Ok(launch) => self.inner.runtime.run(launch).await,
                    Err(error) => Err(error.message),
                };
                match started {
                    Ok(()) => {
                        self.failures().remove(&record.name);
                    }
                    Err(reason) => waiting.push((record, reason)),
                }
            }
            if waiting.is_empty() || waiting.len() == before {
                for (record, reason) in waiting {
                    self.not_started(&record, reason).await;
                }
                return;
            }
            pending = waiting.into_iter().map(|(record, _)| record).collect();
        }
    }

    /// Records once why an enabled plugin does not run.
    async fn not_started(&self, record: &PluginRecord, reason: String) {
        tracing::warn!(plugin = %record.name, %reason, "an enabled plugin did not start");
        let known = self.failures().insert(record.name.clone(), reason.clone());
        if known.as_deref() == Some(reason.as_str()) {
            return;
        }
        let (scope, principal) = system();
        let data = event::PluginDegraded {
            name: record.name.clone(),
            version: record.active_version.clone().unwrap_or_default(),
            reason,
        };
        self.inner
            .events
            .record_by((PLUGIN, &record.name), &scope, &principal, &data)
            .await;
    }

    /// Records the health changes of running plugins until they end.
    pub async fn record_changes(&self, mut changes: mpsc::UnboundedReceiver<Change>) {
        while let Some(change) = changes.recv().await {
            let (scope, principal) = system();
            let events = &self.inner.events;
            match change {
                Change::Degraded {
                    name,
                    version,
                    reason,
                } => {
                    let data = event::PluginDegraded {
                        name: name.clone(),
                        version,
                        reason,
                    };
                    events
                        .record_by((PLUGIN, &name), &scope, &principal, &data)
                        .await;
                }
                Change::Recovered { name, version } => {
                    let data = event::PluginRecovered {
                        name: name.clone(),
                        version,
                    };
                    events
                        .record_by((PLUGIN, &name), &scope, &principal, &data)
                        .await;
                }
                _ => {}
            }
        }
    }

    async fn trusted(&self) -> Result<Vec<TrustedKey>> {
        Ok(self
            .inner
            .store
            .keys()
            .await?
            .into_iter()
            .map(|key| TrustedKey {
                id: key.id,
                public_key: key.public_key,
                comment: key.comment,
            })
            .collect())
    }

    /// Reads the plugins directory again with the keys trusted now.
    async fn rescan(&self) -> Result<(u32, u32)> {
        let keys = self.trusted().await?;
        let root = self.inner.paths.packages.clone();
        let found = tokio::task::spawn_blocking(move || catalog::discover(&root, &keys))
            .await
            .map_err(|_| PanelError::internal("reading the plugins directory stopped"))?
            .map_err(|error| {
                PanelError::unavailable(format!(
                    "the plugins directory {} cannot be read: {error}",
                    self.inner.paths.packages.display()
                ))
            })?;
        let count = |iter: usize| u32::try_from(iter).unwrap_or(u32::MAX);
        let counts = (
            count(found.len()),
            count(found.iter().filter(|found| !found.is_valid()).count()),
        );
        *self
            .inner
            .catalog
            .write()
            .expect("the catalog is never poisoned") = Catalog {
            found,
            discovered_at: Some(Utc::now()),
        };
        Ok(counts)
    }

    async fn discover(&self, cause: Cause<'_>) -> Result<()> {
        let (versions, invalid) = self.rescan().await?;
        self.inner
            .events
            .record_by(
                (PLUGIN, CATALOG),
                cause.scope,
                cause.principal,
                &event::PluginsDiscovered { versions, invalid },
            )
            .await;
        Ok(())
    }

    async fn list(&self) -> Result<PluginList> {
        let records: HashMap<String, PluginRecord> = self
            .inner
            .store
            .plugins()
            .await?
            .into_iter()
            .map(|record| (record.name.clone(), record))
            .collect();
        let catalog = self.catalog();
        let mut names: BTreeSet<&str> = catalog
            .found
            .iter()
            .map(|found| found.name.as_str())
            .collect();
        names.extend(records.keys().map(String::as_str));
        let mut list = PluginList::default();
        list.plugins = names
            .into_iter()
            .map(|name| self.view(name, records.get(name), &catalog))
            .collect();
        list.protocol_versions = HOST_PROTOCOL_VERSIONS.to_vec();
        list.ports = PORTS.iter().map(|port| (*port).to_owned()).collect();
        list.capabilities = PORTS
            .iter()
            .copied()
            .chain([SECRETS_CAPABILITY])
            .map(str::to_owned)
            .collect();
        list.limits_enforced = limits::ENFORCED;
        list.discovered_at = catalog.discovered_at;
        Ok(list)
    }

    async fn plugin_view(&self, name: &str) -> Result<PluginView> {
        let record = self.inner.store.plugin(name).await?;
        let catalog = self.catalog();
        if record.is_none() && catalog.versions(name).next().is_none() {
            return Err(PanelError::not_found(format!("there is no plugin {name}")));
        }
        Ok(self.view(name, record.as_ref(), &catalog))
    }

    fn view(&self, name: &str, record: Option<&PluginRecord>, catalog: &Catalog) -> PluginView {
        let record = record.cloned().unwrap_or_else(|| PluginRecord::new(name));
        let instance = self.inner.runtime.get(name);
        let health = instance
            .as_ref()
            .map(|instance| health_view(instance.version(), instance.health()));
        let state = match (&health, record.enabled) {
            (_, false) => PluginState::Disabled,
            (Some(health), true) if health.status == HealthStatus::Serving => PluginState::Enabled,
            (_, true) => PluginState::Degraded,
        };
        let effective_limits = catalog
            .target(&record)
            .and_then(|found| found.manifest.as_ref())
            .map(|manifest| api_limits(Limits::effective(manifest, &host_limits(record.limits))));
        let error = (record.enabled && instance.is_none())
            .then(|| self.failures().get(name).cloned())
            .flatten();
        let mut view = PluginView::default();
        view.name = name.to_owned();
        view.state = state;
        view.etag = record.etag();
        view.active_version = record.active_version;
        view.previous_version = record.previous_version;
        view.versions = catalog.versions(name).map(version_view).collect();
        view.grants = record.grants;
        view.settings = record.settings;
        view.limits = record.limits;
        view.effective_limits = effective_limits;
        view.health = health;
        view.error = error;
        view.updated_at = record.updated_at;
        view
    }

    /// The process a plugin's record asks for, its secret references
    /// resolved.
    async fn launch(&self, record: &PluginRecord) -> Result<Launch> {
        let name = &record.name;
        let version = record.active_version.as_deref().ok_or_else(|| {
            PanelError::precondition_failed(format!("plugin {name} has no version chosen"))
        })?;
        let found = {
            let catalog = self.catalog();
            runnable(&catalog, name, version)?.clone()
        };
        let manifest = manifest_of(&found).clone();
        let set = host_limits(record.limits);
        let problems = set.problems();
        if !problems.is_empty() {
            return Err(PanelError::validation_failed(problems.join("; ")));
        }
        let schema =
            settings::schema(&manifest.config_schema).map_err(PanelError::validation_failed)?;
        let problems = settings::problems(schema.as_ref(), &record.settings);
        if !problems.is_empty() {
            return Err(PanelError::validation_failed(format!(
                "the settings of plugin {name} {version} do not match its schema: {}",
                problems.join("; ")
            )));
        }
        let granted: Vec<String> = record
            .grants
            .iter()
            .filter(|grant| manifest.capabilities.contains(grant))
            .cloned()
            .collect();
        let references = schema
            .as_ref()
            .map(|schema| settings::references(schema, &record.settings))
            .unwrap_or_default();
        if !references.is_empty() && !granted.iter().any(|grant| grant == SECRETS_CAPABILITY) {
            return Err(PanelError::permission_denied(format!(
                "plugin {name} has not been granted {SECRETS_CAPABILITY}, which the secrets \
                 its settings name need"
            )));
        }
        let mut values = Vec::with_capacity(references.len());
        for (pointer, reference) in references {
            values.push((pointer, self.resolve(name, &reference).await?));
        }
        Ok(Launch {
            executable: found
                .executable()
                .expect("validated versions have executables"),
            directory: found.directory.clone(),
            data: self.inner.paths.data.join(name),
            runtime: self.inner.paths.sockets.clone(),
            limits: Limits::effective(&manifest, &set),
            settings: settings::resolved(&record.settings, &values).to_string(),
            granted,
            manifest,
        })
    }

    /// The value a secret reference names.
    async fn resolve(&self, requester: &str, reference: &str) -> Result<String> {
        let utf8 = |bytes: Vec<u8>, what: &str| {
            String::from_utf8(bytes)
                .map_err(|_| PanelError::validation_failed(format!("secret {what} is not text")))
        };
        match Reference::parse(reference).map_err(PanelError::validation_failed)? {
            Reference::Vault(secret) => {
                let sealed =
                    self.inner.store.sealed(&secret).await?.ok_or_else(|| {
                        PanelError::not_found(format!("there is no secret {secret}"))
                    })?;
                let opened = self.vault()?.open(&owner(&secret), &sealed).await?;
                utf8(opened.to_vec(), reference)
            }
            Reference::Plugin { plugin, path } => {
                if plugin == requester {
                    return Err(PanelError::validation_failed(format!(
                        "plugin {plugin} cannot name its own secrets"
                    )));
                }
                let instance = self.inner.runtime.get(&plugin).ok_or_else(|| {
                    PanelError::unavailable(format!(
                        "plugin {plugin}, which holds secret {path}, does not run"
                    ))
                })?;
                if !instance.provides(SECRETS_PORT) {
                    return Err(PanelError::validation_failed(format!(
                        "plugin {plugin} does not provide secrets"
                    )));
                }
                if !instance.is_granted(SECRETS_PORT) {
                    return Err(PanelError::permission_denied(format!(
                        "plugin {plugin} has not been granted {SECRETS_PORT}"
                    )));
                }
                let (_permit, channel) = instance.admit().map_err(|refusal| {
                    PanelError::unavailable(format!("plugin {plugin} refused: {refusal:?}"))
                })?;
                let timeout = instance.call_timeout();
                let mut request = tonic::Request::new(ResolveRequest { path: path.clone() });
                request.set_timeout(timeout);
                let response = tokio::time::timeout(
                    timeout,
                    SecretProviderClient::new(channel).resolve(request),
                )
                .await
                .map_err(|_| {
                    PanelError::deadline_exceeded(format!(
                        "plugin {plugin} did not resolve secret {path} in time"
                    ))
                })?
                .map_err(|status| {
                    PanelError::unavailable(format!(
                        "plugin {plugin} did not resolve secret {path}: {}",
                        status.message()
                    ))
                })?;
                utf8(response.into_inner().value, reference)
            }
            _ => Err(PanelError::validation_failed(format!(
                "{reference} is not a secret reference this host resolves"
            ))),
        }
    }

    /// Puts the processes in line with `next` after `current`.
    async fn transition(&self, current: &PluginRecord, next: &PluginRecord) -> Result<()> {
        let running = self.inner.runtime.get(&next.name).is_some();
        if next.enabled {
            let unchanged = running
                && current.enabled
                && current.active_version == next.active_version
                && current.grants == next.grants
                && current.settings == next.settings
                && current.limits == next.limits;
            if !unchanged {
                let launch = self.launch(next).await?;
                self.inner.runtime.run(launch).await.map_err(|error| {
                    PanelError::activate_failed(format!(
                        "plugin {} {} did not start: {error}",
                        next.name,
                        next.active_version.as_deref().unwrap_or_default()
                    ))
                })?;
            }
        } else if running {
            self.inner.runtime.stop(&next.name).await;
        }
        Ok(())
    }

    /// Runs what `previous` asked for again, after a change failed to save.
    async fn restore(&self, previous: &PluginRecord) {
        if previous.enabled {
            match self.launch(previous).await {
                Ok(launch) => {
                    if let Err(reason) = self.inner.runtime.run(launch).await {
                        self.not_started(previous, reason).await;
                    }
                }
                Err(error) => {
                    self.inner.runtime.stop(&previous.name).await;
                    self.not_started(previous, error.message).await;
                }
            }
        } else {
            self.inner.runtime.stop(&previous.name).await;
        }
    }

    async fn save(
        &self,
        cause: Cause<'_>,
        record: &PluginRecord,
        recorded: Recorded,
    ) -> Result<()> {
        let aggregate = (PLUGIN, record.name.as_str());
        let events = &self.inner.events;
        macro_rules! envelope {
            ($($variant:ident),*) => {
                match &recorded {
                    $(Recorded::$variant(data) => {
                        events.event_by(aggregate, cause.scope, cause.principal, data)
                    })*
                }
            };
        }
        let envelope =
            envelope!(Granted, Configured, Limited, Enabled, Disabled, Upgraded, RolledBack)?;
        let database = self.inner.store.database();
        let mut transaction = database.begin().await?;
        Store::save(&mut transaction, record).await?;
        SqliteOutbox::append(&mut transaction, &envelope).await?;
        transaction.commit().await.map_err(storage_error)?;
        database.committed();
        Ok(())
    }

    /// Changes one plugin: `edit` turns its record into the next one and
    /// names the event, the processes follow, and the record is saved.
    async fn change_plugin(
        &self,
        cause: Cause<'_>,
        name: &str,
        if_match: Option<&str>,
        edit: impl FnOnce(&mut PluginRecord, &Catalog) -> Result<Recorded>,
    ) -> Result<PluginOutput> {
        let stored = self.inner.store.plugin(name).await?;
        let current = {
            let catalog = self.catalog();
            if stored.is_none() && catalog.versions(name).next().is_none() {
                return Err(PanelError::not_found(format!("there is no plugin {name}")));
            }
            stored.unwrap_or_else(|| PluginRecord::new(name))
        };
        if let Some(tag) = if_match.filter(|tag| *tag != current.etag()) {
            return Err(PanelError::precondition_failed(format!(
                "plugin {name} has changed; it is at {}, not {tag}",
                current.etag()
            )));
        }
        let mut next = current.clone();
        let recorded = {
            let catalog = self.catalog();
            edit(&mut next, &catalog)?
        };
        next.version = current.version + 1;
        next.updated_at = Some(Utc::now());
        self.transition(&current, &next).await?;
        if let Err(error) = self.save(cause, &next, recorded).await {
            self.restore(&current).await;
            return Err(error);
        }
        self.failures().remove(name);
        let view = self.plugin_view(name).await?;
        let etag = view.etag.clone();
        Ok(output(&view, Some(etag)))
    }

    async fn apply(
        &self,
        cause: Cause<'_>,
        command: PluginCommand,
        if_match: Option<&str>,
    ) -> Result<PluginOutput> {
        match command {
            PluginCommand::Discover => {
                self.discover(cause).await?;
                Ok(output(&self.list().await?, None))
            }
            PluginCommand::Grant { name, capabilities } => {
                self.change_plugin(cause, &name, if_match, |record, catalog| {
                    grant(record, catalog, capabilities)
                })
                .await
            }
            PluginCommand::Configure { name, settings } => {
                self.change_plugin(cause, &name, if_match, |record, catalog| {
                    configure(record, catalog, settings)
                })
                .await
            }
            PluginCommand::Limit { name, limits } => {
                self.change_plugin(cause, &name, if_match, |record, _| {
                    let problems = host_limits(limits).problems();
                    if !problems.is_empty() {
                        return Err(PanelError::validation_failed(problems.join("; ")));
                    }
                    record.limits = limits;
                    Ok(Recorded::Limited(event::PluginLimited {
                        name: record.name.clone(),
                        memory_bytes: limits.memory_bytes,
                        cpu_seconds: limits.cpu_seconds,
                        open_files: limits.open_files,
                        concurrency: limits.concurrency,
                        call_timeout_ms: limits.call_timeout_ms,
                    }))
                })
                .await
            }
            PluginCommand::Enable { name, version } => {
                self.change_plugin(cause, &name, if_match, |record, catalog| {
                    enable(record, catalog, version)
                })
                .await
            }
            PluginCommand::Disable { name } => {
                self.change_plugin(cause, &name, if_match, |record, _| {
                    record.enabled = false;
                    Ok(Recorded::Disabled(event::PluginDisabled {
                        name: record.name.clone(),
                        version: record.active_version.clone().unwrap_or_default(),
                    }))
                })
                .await
            }
            PluginCommand::Upgrade { name, version } => {
                self.change_plugin(cause, &name, if_match, |record, catalog| {
                    upgrade(record, catalog, version)
                })
                .await
            }
            PluginCommand::Rollback { name } => {
                self.change_plugin(cause, &name, if_match, |record, catalog| {
                    rollback(record, catalog)
                })
                .await
            }
            PluginCommand::PutKey { key } => {
                let trusted = self.put_key(cause, key).await?;
                Ok(output(&trusted, None))
            }
            PluginCommand::DeleteKey { id } => {
                self.delete_key(cause, &id).await?;
                Ok(nothing())
            }
            PluginCommand::PutSecret { name, value } => {
                let kept = self.put_secret(cause, &name, &value).await?;
                Ok(output(&kept, None))
            }
            PluginCommand::DeleteSecret { name } => {
                self.delete_secret(cause, &name).await?;
                Ok(nothing())
            }
        }
    }

    async fn put_key(&self, cause: Cause<'_>, key: NewTrustedKey) -> Result<TrustedKeyView> {
        if !manifest::is_name(&key.id) {
            return Err(PanelError::invalid_argument(format!(
                "{:?} is not a key ID: use lowercase letters, digits and hyphens",
                key.id
            )));
        }
        let (public_key, key_id) =
            signature::public_key(&key.public_key).map_err(PanelError::invalid_argument)?;
        let mut trusted = TrustedKeyView::default();
        trusted.id = key.id;
        trusted.key_id = key_id;
        trusted.public_key = public_key;
        trusted.comment = key.comment.trim().to_owned();
        trusted.created_at = Utc::now();
        let data = event::KeyTrusted {
            id: trusted.id.clone(),
            key_id: trusted.key_id.clone(),
        };
        let envelope =
            self.inner
                .events
                .event_by((KEY, &trusted.id), cause.scope, cause.principal, &data)?;
        let database = self.inner.store.database();
        let mut transaction = database.begin().await?;
        Store::put_key(&mut transaction, &trusted).await?;
        SqliteOutbox::append(&mut transaction, &envelope).await?;
        transaction.commit().await.map_err(storage_error)?;
        database.committed();
        self.rescan().await?;
        Ok(trusted)
    }

    async fn delete_key(&self, cause: Cause<'_>, id: &str) -> Result<()> {
        for record in self.inner.store.plugins().await? {
            let catalog = self.catalog();
            let signs = record.enabled
                && record
                    .active_version
                    .as_deref()
                    .and_then(|version| catalog.version(&record.name, version))
                    .is_some_and(|found| found.signed_by.as_deref() == Some(id));
            if signs {
                return Err(PanelError::conflict(format!(
                    "key {id} signs plugin {} {}, which is enabled: disable it first",
                    record.name,
                    record.active_version.unwrap_or_default()
                )));
            }
        }
        let envelope = self.inner.events.event_by(
            (KEY, id),
            cause.scope,
            cause.principal,
            &event::KeyRemoved { id: id.to_owned() },
        )?;
        let database = self.inner.store.database();
        let mut transaction = database.begin().await?;
        Store::delete_key(&mut transaction, id).await?;
        SqliteOutbox::append(&mut transaction, &envelope).await?;
        transaction.commit().await.map_err(storage_error)?;
        database.committed();
        self.rescan().await?;
        Ok(())
    }

    async fn put_secret(&self, cause: Cause<'_>, name: &str, value: &Secret) -> Result<SecretView> {
        if !manifest::is_name(name) {
            return Err(PanelError::invalid_argument(format!(
                "{name:?} is not a secret name: use lowercase letters, digits and hyphens"
            )));
        }
        if value.expose().is_empty() {
            return Err(PanelError::invalid_argument("a secret cannot be empty"));
        }
        let sealed = self
            .vault()?
            .seal(&owner(name), value.expose().as_bytes())
            .await?;
        let now = Utc::now();
        let envelope = self.inner.events.event_by(
            (SECRET, name),
            cause.scope,
            cause.principal,
            &event::SecretSealed {
                name: name.to_owned(),
            },
        )?;
        let database = self.inner.store.database();
        let mut transaction = database.begin().await?;
        Store::put_secret(&mut transaction, name, &sealed, now).await?;
        SqliteOutbox::append(&mut transaction, &envelope).await?;
        transaction.commit().await.map_err(storage_error)?;
        database.committed();
        let mut kept = SecretView::default();
        kept.name = name.to_owned();
        kept.updated_at = now;
        Ok(kept)
    }

    async fn delete_secret(&self, cause: Cause<'_>, name: &str) -> Result<()> {
        let reference = format!("vault:{name}");
        if let Some(user) = self
            .inner
            .store
            .plugins()
            .await?
            .into_iter()
            .find(|record| names_secret(&record.settings, &reference))
        {
            return Err(PanelError::conflict(format!(
                "the settings of plugin {} name secret {name}",
                user.name
            )));
        }
        let envelope = self.inner.events.event_by(
            (SECRET, name),
            cause.scope,
            cause.principal,
            &event::SecretDeleted {
                name: name.to_owned(),
            },
        )?;
        let database = self.inner.store.database();
        let mut transaction = database.begin().await?;
        Store::delete_secret(&mut transaction, name).await?;
        SqliteOutbox::append(&mut transaction, &envelope).await?;
        transaction.commit().await.map_err(storage_error)?;
        database.committed();
        Ok(())
    }
}

fn grant(
    record: &mut PluginRecord,
    catalog: &Catalog,
    capabilities: Vec<String>,
) -> Result<Recorded> {
    let asked: BTreeSet<&str> = catalog
        .versions(&record.name)
        .filter_map(|found| found.manifest.as_ref())
        .flat_map(|manifest| manifest.capabilities.iter().map(String::as_str))
        .collect();
    for capability in &capabilities {
        if !manifest::is_capability(capability) {
            return Err(PanelError::invalid_argument(format!(
                "{capability} is not a capability"
            )));
        }
        if !asked.contains(capability.as_str()) {
            return Err(PanelError::invalid_argument(format!(
                "plugin {} does not ask for {capability}",
                record.name
            )));
        }
    }
    let granted: BTreeSet<String> = capabilities.into_iter().collect();
    record.grants = granted.into_iter().collect();
    Ok(Recorded::Granted(event::PluginGranted {
        name: record.name.clone(),
        capabilities: record.grants.clone(),
    }))
}

fn configure(record: &mut PluginRecord, catalog: &Catalog, settings: Value) -> Result<Recorded> {
    let name = record.name.clone();
    let Value::Object(fields) = &settings else {
        return Err(PanelError::invalid_argument("settings are a JSON object"));
    };
    let found = catalog
        .target(record)
        .ok_or_else(|| PanelError::not_found(format!("no version of plugin {name} can run")))?;
    let manifest = found.manifest.as_ref().ok_or_else(|| {
        PanelError::validation_failed(format!("plugin {name} {} cannot run", found.version))
    })?;
    let schema =
        settings::schema(&manifest.config_schema).map_err(PanelError::validation_failed)?;
    let problems = settings::problems(schema.as_ref(), &settings);
    if !problems.is_empty() {
        return Err(PanelError::validation_failed(format!(
            "the settings do not match the schema of plugin {name} {}: {}",
            found.version,
            problems.join("; ")
        )));
    }
    let references = schema
        .as_ref()
        .map(|schema| settings::references(schema, &settings))
        .unwrap_or_default();
    let data = event::PluginConfigured {
        name,
        keys: fields.keys().cloned().collect(),
        references: references.into_iter().map(|(pointer, _)| pointer).collect(),
    };
    record.settings = settings;
    Ok(Recorded::Configured(data))
}

fn enable(
    record: &mut PluginRecord,
    catalog: &Catalog,
    version: Option<String>,
) -> Result<Recorded> {
    let name = record.name.clone();
    let chosen = match version {
        Some(version) => version,
        None => record
            .active_version
            .clone()
            .filter(|version| catalog.version(&name, version).is_some_and(Found::is_valid))
            .or_else(|| {
                catalog
                    .newest_valid(&name)
                    .map(|found| found.version.clone())
            })
            .ok_or_else(|| PanelError::not_found(format!("no version of plugin {name} can run")))?,
    };
    runnable(catalog, &name, &chosen)?;
    if record.active_version.as_deref() != Some(chosen.as_str()) {
        record.previous_version = record.active_version.take();
    }
    record.active_version = Some(chosen.clone());
    record.enabled = true;
    Ok(Recorded::Enabled(event::PluginEnabled {
        name,
        version: chosen,
    }))
}

fn upgrade(record: &mut PluginRecord, catalog: &Catalog, version: String) -> Result<Recorded> {
    let name = record.name.clone();
    let from = record.active_version.clone().ok_or_else(|| {
        PanelError::precondition_failed(format!(
            "plugin {name} has no version to upgrade from: enable it"
        ))
    })?;
    if from == version {
        return Err(PanelError::invalid_argument(format!(
            "plugin {name} is at {version} already"
        )));
    }
    runnable(catalog, &name, &version)?;
    record.previous_version = Some(from.clone());
    record.active_version = Some(version.clone());
    Ok(Recorded::Upgraded(event::PluginUpgraded {
        name,
        from_version: from,
        to_version: version,
    }))
}

fn rollback(record: &mut PluginRecord, catalog: &Catalog) -> Result<Recorded> {
    let name = record.name.clone();
    let to = record.previous_version.clone().ok_or_else(|| {
        PanelError::precondition_failed(format!("plugin {name} has no version to roll back to"))
    })?;
    runnable(catalog, &name, &to)?;
    let from = record.active_version.clone().unwrap_or_default();
    record.previous_version = record.active_version.take();
    record.active_version = Some(to.clone());
    Ok(Recorded::RolledBack(event::PluginRolledBack {
        name,
        from_version: from,
        to_version: to,
    }))
}

#[async_trait]
impl PluginsPort for PluginService {
    async fn read(&self, _scope: RequestScope, query: PluginQuery) -> Result<PluginOutput> {
        Ok(match query {
            PluginQuery::Plugins => output(&self.list().await?, None),
            PluginQuery::Plugin { name } => {
                let view = self.plugin_view(&name).await?;
                let etag = view.etag.clone();
                output(&view, Some(etag))
            }
            PluginQuery::Keys => output(&self.inner.store.keys().await?, None),
            PluginQuery::Secrets => output(&self.inner.store.secrets().await?, None),
        })
    }

    async fn change(&self, context: CommandContext, change: PluginChange) -> Result<PluginOutput> {
        let scope = context.scope();
        let principal = EventLog::user(context.actor());
        let cause = Cause {
            scope: &scope,
            principal: &principal,
        };
        let operation = change.command.operation();
        let (kind, id) = change.command.resource();
        let result = {
            let _change = self.inner.changes.lock().await;
            self.apply(cause, change.command, change.if_match.as_deref())
                .await
        };
        if let Err(error) = &result {
            let aggregate = match kind {
                "plugin-keys" => KEY,
                "plugin-secrets" => SECRET,
                _ => PLUGIN,
            };
            let id = if id.is_empty() {
                CATALOG.to_owned()
            } else {
                id
            };
            let data = event::ChangeRefused {
                operation: operation.to_owned(),
                resource: id.clone(),
                reason: error.message.clone(),
                code: error.code.as_str().to_owned(),
            };
            self.inner
                .events
                .record_by((aggregate, &id), cause.scope, cause.principal, &data)
                .await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_references_are_found_wherever_settings_hold_them() {
        let settings = serde_json::json!({"a": {"b": ["x", "vault:token"]}, "c": "vault:other"});
        assert!(names_secret(&settings, "vault:token"));
        assert!(names_secret(&settings, "vault:other"));
        assert!(!names_secret(&settings, "vault:tok"));
    }
}
