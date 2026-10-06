use crate::{
    access_log::LoggingPlan,
    acme::ChallengeDirectory,
    certificates::CertificateIndex,
    file_checks::{self, FileChecks},
    http_policy::HttpPolicy,
    listeners::{self, ListenerPlan, SocketKey},
    lua::{self, LuaPlan},
    routing::{RoutingTable, Targets},
    secrets::{NoSecrets, SecretSource},
    security::{LimitState, SecurityGate},
    static_files::StaticContent,
    telemetry::SnapshotLabels,
    upstream::{EndpointStates, PoolHealth, UpstreamPool},
    ADAPTER_VERSION, PINGORA_PACKAGE_VERSION,
};
use arc_swap::{ArcSwap, ArcSwapOption};
use async_trait::async_trait;
use panel_domain::RevisionId;
use panel_engine::{validate_engine_ir, DataPlaneAdapter, EngineCapabilities, EngineCapability};
use panel_errors::{Diagnostic, ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use parking_lot::Mutex;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

pub(crate) type ActiveSnapshot = Arc<ArcSwapOption<PreparedPingoraSnapshot>>;

/// Capabilities this adapter serves. HTTP/3 listeners and Unix socket
/// upstreams are reserved in the IR and deliberately absent.
const CAPABILITIES: &[&str] = &[
    "action.redirect",
    "action.respond",
    "action.static",
    "action.template",
    "http.policies",
    "activation.cas",
    "listener.http",
    "listener.http2",
    "listener.https",
    "listener.request-head-timeout",
    "listener.tls-settings",
    "listener.trusted-proxies",
    "log.access",
    "lua.scripts",
    "request.security",
    "response.hsts",
    "route.conditions",
    "route.exact-path",
    "route.glob",
    "route.named",
    "route.host",
    "route.path-prefix",
    "route.regex",
    "site.redirect",
    "upstream.backup",
    "upstream.balancing",
    "upstream.health-check",
    "upstream.http",
    "upstream.http2",
    "upstream.https",
    "upstream.passive-health",
    "upstream.resilience",
];

/// Gateway resources the adapter reads while preparing snapshots.
#[derive(Clone)]
pub struct AdapterOptions {
    secrets: Arc<dyn SecretSource>,
    static_root: Option<PathBuf>,
    challenges: Option<Arc<ChallengeDirectory>>,
    lua_vms: usize,
}

impl Default for AdapterOptions {
    fn default() -> Self {
        Self {
            secrets: Arc::new(NoSecrets),
            static_root: None,
            challenges: None,
            lua_vms: std::thread::available_parallelism().map_or(1, usize::from),
        }
    }
}

impl fmt::Debug for AdapterOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdapterOptions")
            .field("static_root", &self.static_root)
            .field("challenges", &self.challenges)
            .finish_non_exhaustive()
    }
}

impl AdapterOptions {
    /// Source of TLS certificates, keys and CA bundles named by snapshots.
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretSource>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Where HTTP-01 key authorizations wait while certificates are issued.
    pub fn with_challenges(mut self, challenges: ChallengeDirectory) -> Self {
        self.challenges = Some(Arc::new(challenges));
        self
    }

    /// Directory that static content roots are relative to.
    pub fn with_static_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.static_root = Some(root.into());
        self
    }

    /// Lua VMs each snapshot starts: one per data-plane worker thread.
    pub fn with_lua_vms(mut self, vms: usize) -> Self {
        self.lua_vms = vms.max(1);
        self
    }
}

pub struct PingoraGatewayAdapter {
    active: ActiveSnapshot,
    activations: watch::Sender<u64>,
    /// When the active snapshot was activated, in milliseconds since the
    /// Unix epoch; zero before the first activation.
    activated_at: AtomicU64,
    options: AdapterOptions,
    endpoints: Arc<EndpointStates>,
    bound: Mutex<BTreeSet<SocketKey>>,
    /// Rate limit and concurrency state that outlives snapshots.
    limits: Arc<LimitState>,
    /// `ngx.shared` dictionaries, which keep their contents across
    /// snapshots.
    lua_dicts: Arc<panel_lua::SharedStore>,
}

/// Opaque immutable artifact built entirely before activation.
///
/// Its fields remain private so no upstream Pingora value can cross the adapter
/// boundary even though the associated type is visible to the generic runtime.
pub struct PreparedPingoraSnapshot {
    snapshot: RuntimeSnapshot,
    pub(crate) routing: RoutingTable,
    pub(crate) pools: Vec<UpstreamPool>,
    pub(crate) statics: Vec<StaticContent>,
    /// Compiled security policies, by the indexes sites and routes hold.
    pub(crate) policies: Vec<SecurityGate>,
    /// Compiled HTTP policies, by the indexes sites and routes hold.
    pub(crate) http: Vec<HttpPolicy>,
    /// Replaced when the certificate files change, without a new snapshot.
    pub(crate) certificates: ArcSwap<CertificateIndex>,
    pub(crate) listeners: Vec<ListenerPlan>,
    pub(crate) labels: SnapshotLabels,
    /// What records keep out and how log files rotate.
    pub(crate) logging: LoggingPlan,
    /// The VMs of the snapshot's scripts.
    pub(crate) lua: Option<Arc<LuaPlan>>,
    /// The TLS listeners some of whose sites run TLS handshake scripts.
    pub(crate) scripted_handshakes: HashSet<String>,
}

impl Default for PingoraGatewayAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PingoraGatewayAdapter {
    pub fn new() -> Self {
        Self::with_options(AdapterOptions::default())
    }

    pub fn with_options(options: AdapterOptions) -> Self {
        Self {
            active: Arc::new(ArcSwapOption::empty()),
            activations: watch::channel(0).0,
            activated_at: AtomicU64::new(0),
            options,
            endpoints: Arc::default(),
            bound: Mutex::default(),
            limits: Arc::default(),
            lua_dicts: Arc::default(),
        }
    }

    pub fn pingora_package_version(&self) -> &'static str {
        PINGORA_PACKAGE_VERSION
    }

    /// Serves the certificate files of the active snapshot again when they
    /// changed on disk, such as after a renewal, without a new revision.
    /// When the new files do not load, the current certificates stay and the
    /// error is returned. Returns whether the certificates were replaced.
    pub async fn reload_certificates(&self) -> Result<bool> {
        let Some(active) = self.active.load_full() else {
            return Ok(false);
        };
        let secrets = Arc::clone(&self.options.secrets);
        let current = Arc::clone(&active);
        let reloaded = tokio::task::spawn_blocking(move || {
            let material = CertificateIndex::material(&current.snapshot, secrets.as_ref())?;
            if current.certificates.load().built_from(&material) {
                return Ok(None);
            }
            CertificateIndex::build(&current.snapshot, secrets.as_ref()).map(Some)
        })
        .await
        .map_err(|error| PanelError::internal(format!("certificate reload stopped: {error}")))??;
        Ok(match reloaded {
            Some(index) => {
                active.certificates.store(Arc::new(index));
                true
            }
            None => false,
        })
    }

    pub fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    /// Bytes each Lua VM of the active snapshot uses.
    pub fn lua_memory(&self) -> Vec<usize> {
        self.active
            .load()
            .as_ref()
            .and_then(|prepared| prepared.lua.as_ref())
            .map(|plan| plan.runtime.memory())
            .unwrap_or_default()
    }

    /// The revision of the active snapshot.
    pub fn active_revision(&self) -> Option<RevisionId> {
        self.active
            .load()
            .as_ref()
            .map(|prepared| prepared.snapshot.revision_id)
    }

    /// When the active snapshot was activated.
    pub fn activated_at(&self) -> Option<SystemTime> {
        match self.activated_at.load(Relaxed) {
            0 => None,
            millis => Some(UNIX_EPOCH + Duration::from_millis(millis)),
        }
    }

    pub fn active_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.active
            .load_full()
            .map(|prepared| prepared.snapshot.clone())
    }

    /// What the files the active snapshot serves from look like; reads the
    /// file system, so call it off the request threads.
    pub fn check_files(&self) -> FileChecks {
        let snapshot = self.active_snapshot();
        file_checks::check(
            snapshot.as_ref(),
            self.options.secrets.as_ref(),
            self.options.static_root.as_deref(),
        )
    }

    /// Live health of every upstream pool in the active snapshot.
    pub fn upstream_health(&self) -> Vec<PoolHealth> {
        self.active
            .load_full()
            .map(|prepared| prepared.pools.iter().map(UpstreamPool::health).collect())
            .unwrap_or_default()
    }

    /// Takes an endpoint out of rotation, or returns it, until changed again.
    pub fn set_endpoint_drained(&self, pool: &str, endpoint: &str, drained: bool) -> Result<()> {
        let exists = self.active.load().as_ref().is_some_and(|prepared| {
            prepared.pools.iter().any(|candidate| {
                candidate.id.as_str() == pool
                    && candidate
                        .endpoints
                        .iter()
                        .any(|item| item.id.as_str() == endpoint)
            })
        });
        if !exists {
            return Err(PanelError::not_found(format!(
                "the active configuration has no endpoint {endpoint} in upstream {pool}"
            )));
        }
        self.endpoints.set_drained(pool, endpoint, drained);
        Ok(())
    }

    /// Endpoints currently drained by an operator, as `(pool, endpoint)`.
    pub fn drained_endpoints(&self) -> BTreeSet<(String, String)> {
        self.endpoints.drained()
    }

    /// Restores drains recorded before a restart; unknown endpoints are kept
    /// until the next activation shows whether they still exist.
    pub fn restore_drained(&self, drained: impl IntoIterator<Item = (String, String)>) {
        for (pool, endpoint) in drained {
            self.endpoints.set_drained(&pool, &endpoint, true);
        }
    }

    pub(crate) fn active(&self) -> ActiveSnapshot {
        Arc::clone(&self.active)
    }

    pub(crate) fn challenges(&self) -> Option<Arc<ChallengeDirectory>> {
        self.options.challenges.clone()
    }

    pub(crate) fn active_prepared(&self) -> Option<Arc<PreparedPingoraSnapshot>> {
        self.active.load_full()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.activations.subscribe()
    }

    pub(crate) fn set_bound_sockets(&self, sockets: BTreeSet<SocketKey>) {
        *self.bound.lock() = sockets;
    }

    fn supported_capabilities() -> BTreeSet<EngineCapability> {
        CAPABILITIES
            .iter()
            .map(|name| EngineCapability::new(*name, "1"))
            .collect()
    }

    fn validate_supported_ir(snapshot: &RuntimeSnapshot) -> Result<()> {
        let mut unsupported = Vec::new();
        if !snapshot.cache_policies.is_empty() {
            unsupported.push("cache_policies");
        }
        if !snapshot.lua_policies.is_empty() {
            unsupported.push("lua_policies");
        }
        if snapshot
            .listeners
            .iter()
            .any(|listener| listener.protocols.http3)
        {
            unsupported.push("HTTP/3 listeners (reserved)");
        }
        for route in &snapshot.routes {
            if route.retry_policy.is_some()
                || route.cache_policy_id.is_some()
                || route.lua_policy_id.is_some()
            {
                unsupported.push("route policy");
            }
        }
        for pool in &snapshot.upstream_pools {
            if pool
                .endpoints
                .iter()
                .any(|endpoint| endpoint.unix_socket.is_some())
            {
                unsupported.push("Unix socket upstreams (reserved)");
            }
        }
        if unsupported.is_empty() {
            return Ok(());
        }
        unsupported.sort_unstable();
        unsupported.dedup();
        Err(PanelError::unsupported_capability(format!(
            "Pingora adapter does not support IR nodes: {}",
            unsupported.join(", ")
        )))
    }

    async fn compile(&self, snapshot: RuntimeSnapshot) -> Result<PreparedPingoraSnapshot> {
        let listeners = snapshot
            .listeners
            .iter()
            .map(|listener| ListenerPlan::from_ir(listener, &snapshot.tls_profiles))
            .collect::<Result<Vec<_>>>()?;
        self.check_bindable(&listeners)?;
        let secrets = Arc::clone(&self.options.secrets);
        let static_root = self.options.static_root.clone();
        let blocking = snapshot.clone();
        let limits = Arc::clone(&self.limits);
        let lua_dicts = Arc::clone(&self.lua_dicts);
        let lua_vms = self.options.lua_vms;
        let (certificates, statics, policies, (lua, lua_hooks)) =
            tokio::task::spawn_blocking(move || {
                let certificates = CertificateIndex::build(&blocking, secrets.as_ref())?;
                let statics = blocking
                    .static_content
                    .iter()
                    .map(|policy| StaticContent::compile(policy, static_root.as_deref()))
                    .collect::<Result<Vec<_>>>()?;
                let policies = blocking
                    .security_policies
                    .iter()
                    .map(|policy| SecurityGate::compile(policy, secrets.as_ref(), &limits))
                    .collect::<Result<Vec<_>>>()?;
                let lua = lua::compile(&blocking, &lua_dicts, lua_vms, secrets.as_ref())?;
                Ok::<_, PanelError>((certificates, statics, policies, lua))
            })
            .await
            .map_err(|error| {
                PanelError::internal(format!("snapshot compilation stopped: {error}"))
            })??;
        let mut pools = Vec::with_capacity(snapshot.upstream_pools.len());
        for pool in &snapshot.upstream_pools {
            let mut compiled =
                UpstreamPool::compile(pool, &self.endpoints, self.options.secrets.as_ref()).await?;
            compiled.balancer = lua_hooks.balancers.get(pool.id.as_str()).cloned();
            pools.push(compiled);
        }
        let pool_indexes: HashMap<&str, usize> = snapshot
            .upstream_pools
            .iter()
            .enumerate()
            .map(|(index, pool)| (pool.id.as_str(), index))
            .collect();
        let static_indexes: HashMap<&str, usize> = snapshot
            .static_content
            .iter()
            .enumerate()
            .map(|(index, policy)| (policy.id.as_str(), index))
            .collect();
        let policy_indexes: HashMap<&str, usize> = snapshot
            .security_policies
            .iter()
            .enumerate()
            .map(|(index, policy)| (policy.id.as_str(), index))
            .collect();
        let http = snapshot
            .header_policies
            .iter()
            .map(HttpPolicy::compile)
            .collect::<Result<Vec<_>>>()?;
        let http_indexes: HashMap<&str, usize> = snapshot
            .header_policies
            .iter()
            .enumerate()
            .map(|(index, policy)| (policy.id.as_str(), index))
            .collect();
        let routing = RoutingTable::compile(
            &snapshot,
            &Targets {
                pools: &pool_indexes,
                statics: &static_indexes,
                policies: &policy_indexes,
                http: &http_indexes,
                lua: &lua_hooks,
            },
        )?;
        let labels = SnapshotLabels::new(&routing, &pools);
        let logging = LoggingPlan::new(&snapshot.logging, Some(snapshot.revision_id.get()));
        let fetches = lua.as_ref().is_some_and(|lua| lua.session_fetch.is_some());
        let scripted_handshakes = listeners
            .iter()
            .filter(|plan| plan.tls)
            .filter(|plan| {
                (fetches && plan.handshake.session_resumption)
                    || routing.sites().iter().enumerate().any(|(index, site)| {
                        routing.serves(index, &plan.id)
                            && (site.lua.ssl_client_hello.is_some() || site.lua.ssl_cert.is_some())
                    })
            })
            .map(|plan| plan.id.clone())
            .collect();
        Ok(PreparedPingoraSnapshot {
            snapshot,
            routing,
            pools,
            statics,
            policies,
            http,
            certificates: ArcSwap::from_pointee(certificates),
            listeners,
            labels,
            logging,
            lua,
            scripted_handshakes,
        })
    }

    /// Detects port conflicts with other processes before activation. Ports
    /// this gateway holds, or binds for the active snapshot once the data
    /// plane catches up with an activation, are resolved when the listener
    /// set changes.
    fn check_bindable(&self, listeners: &[ListenerPlan]) -> Result<()> {
        let mut bound = self.bound.lock().clone();
        if let Some(active) = self.active_prepared() {
            bound.extend(active.listeners.iter().map(|plan| plan.socket.clone()));
        }
        let diagnostics: Vec<_> = listeners
            .iter()
            .filter(|plan| {
                !bound.contains(&plan.socket)
                    && !bound
                        .iter()
                        .any(|held| held.address.port() == plan.socket.address.port())
            })
            .filter_map(|plan| {
                listeners::bind(&plan.socket).err().map(|error| {
                    Diagnostic::error(
                        ErrorCode::VALIDATION_FAILED,
                        format!(
                            "listener {} cannot use {}: {error}",
                            plan.id, plan.socket.address
                        ),
                    )
                    .with_resource(plan.id.clone())
                })
            })
            .collect();
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(PanelError::new(
                ErrorCode::VALIDATION_FAILED,
                "listener ports are unavailable",
            )
            .with_diagnostics(diagnostics))
        }
    }
}

#[async_trait]
impl DataPlaneAdapter for PingoraGatewayAdapter {
    type Prepared = PreparedPingoraSnapshot;

    async fn capabilities(&self) -> Result<EngineCapabilities> {
        Ok(EngineCapabilities {
            protocol_version: "pingora.panel.gateway.v1".into(),
            build_version: PINGORA_PACKAGE_VERSION.into(),
            schema_version: panel_ir::IR_SCHEMA_VERSION.into(),
            adapter_version: ADAPTER_VERSION.into(),
            capabilities: Self::supported_capabilities(),
        })
    }

    async fn validate(&self, snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
        let base = validate_engine_ir(snapshot, &Self::supported_capabilities())?;
        if !base.valid {
            return Ok(base);
        }
        Self::validate_supported_ir(snapshot)?;
        Ok(base)
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<Self::Prepared> {
        let report = self.validate(&snapshot).await?;
        if !report.valid {
            return Err(PanelError::new(
                ErrorCode::VALIDATION_FAILED,
                "snapshot validation failed",
            )
            .with_diagnostics(report.diagnostics));
        }
        self.compile(snapshot).await
    }

    fn activate(&self, prepared: Arc<Self::Prepared>) {
        let live: HashSet<(String, String)> = prepared
            .pools
            .iter()
            .flat_map(|pool| {
                pool.endpoints
                    .iter()
                    .map(|endpoint| (pool.id.as_str().to_owned(), endpoint.id.as_str().to_owned()))
            })
            .collect();
        self.active.store(Some(prepared));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        self.activated_at.store(now.max(1), Relaxed);
        self.endpoints.retain(&live);
        self.activations.send_modify(|generation| *generation += 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{EndpointAddress, EndpointId, UpstreamPoolId};
    use panel_ir::{
        CachePolicy, CapabilityRequirement, ListenerRef, UpstreamEndpoint, UpstreamPoolSpec,
    };

    fn mapped_snapshot(tls: bool) -> RuntimeSnapshot {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut endpoint = UpstreamEndpoint::new(
            EndpointId::new("local").unwrap(),
            EndpointAddress::new("127.0.0.1", if tls { 443 } else { 80 }, tls).unwrap(),
        );
        endpoint.sni = tls.then(|| "example.com".into());
        snapshot.upstream_pools.push(UpstreamPoolSpec::new(
            UpstreamPoolId::new("primary").unwrap(),
            "primary",
            vec![endpoint],
        ));
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(
                if tls {
                    "upstream.https"
                } else {
                    "upstream.http"
                },
                "1",
            ));
        snapshot.refresh_content_hash();
        snapshot
    }

    #[tokio::test]
    async fn maps_http_and_https_peers() {
        let adapter = PingoraGatewayAdapter::new();
        for tls in [false, true] {
            let snapshot = mapped_snapshot(tls);
            assert!(adapter.validate(&snapshot).await.unwrap().valid);
            adapter.prepare(snapshot).await.unwrap();
        }
    }

    #[tokio::test]
    async fn unsupported_and_reserved_nodes_fail_explicitly() {
        let adapter = PingoraGatewayAdapter::new();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.cache_policies.push(CachePolicy {
            id: "cache".into(),
            enabled: true,
            ttl_seconds: 60,
            vary_headers: BTreeSet::new(),
        });
        snapshot.refresh_content_hash();
        let error = adapter.validate(&snapshot).await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);

        let mut snapshot = mapped_snapshot(false);
        let mut listener = ListenerRef::new("quic", "127.0.0.1:8443");
        listener.protocols.http3 = true;
        snapshot.listeners.push(listener);
        snapshot.upstream_pools[0].endpoints[0].unix_socket = Some("/run/app.sock".into());
        snapshot.refresh_content_hash();
        let error = adapter.validate(&snapshot).await.unwrap_err();
        assert!(
            error.message.contains("HTTP/3 listeners (reserved)"),
            "{error}"
        );
        assert!(
            error.message.contains("Unix socket upstreams (reserved)"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn occupied_listener_ports_fail_preparation() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.listeners.push(ListenerRef::new(
            "http",
            occupied.local_addr().unwrap().to_string(),
        ));
        snapshot.refresh_content_hash();
        let error = PingoraGatewayAdapter::new()
            .prepare(snapshot)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code.as_str(), ErrorCode::VALIDATION_FAILED);
        assert!(error.diagnostics[0]
            .message
            .contains("listener http cannot use"));
    }

    #[tokio::test]
    async fn ports_of_the_active_snapshot_count_as_held() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = free.local_addr().unwrap().to_string();
        drop(free);
        let snapshot = |revision| {
            let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(revision));
            snapshot
                .listeners
                .push(ListenerRef::new("http", address.clone()));
            snapshot.refresh_content_hash();
            snapshot
        };
        let adapter = PingoraGatewayAdapter::new();
        adapter.activate(Arc::new(adapter.prepare(snapshot(1)).await.unwrap()));
        // The data plane binds the active listeners before it reports them.
        let _held = std::net::TcpListener::bind(&address).unwrap();
        adapter.prepare(snapshot(2)).await.unwrap();
    }

    /// Reserved feature gates must never widen the advertised capability set,
    /// so the adapter reports exactly these capabilities under every selection.
    #[tokio::test]
    async fn reserved_feature_gates_do_not_widen_advertised_capabilities() {
        let advertised = PingoraGatewayAdapter::new()
            .capabilities()
            .await
            .unwrap()
            .capabilities;
        assert_eq!(advertised.len(), CAPABILITIES.len());
        assert!(!advertised.contains(&EngineCapability::new("listener.http3", "1")));
        assert!(!advertised.contains(&EngineCapability::new("upstream.unix", "1")));
        assert!(advertised.contains(&EngineCapability::new("activation.cas", "1")));
    }

    #[tokio::test]
    async fn activation_publishes_and_drains_are_scoped_to_active_endpoints() {
        let adapter = PingoraGatewayAdapter::new();
        let activations = adapter.subscribe();
        assert!(adapter
            .set_endpoint_drained("primary", "local", true)
            .is_err());
        let prepared = Arc::new(adapter.prepare(mapped_snapshot(false)).await.unwrap());
        adapter.activate(prepared);
        assert!(activations.has_changed().unwrap());
        assert!(adapter.active_snapshot().is_some());
        adapter
            .set_endpoint_drained("primary", "local", true)
            .unwrap();
        assert_eq!(
            adapter.drained_endpoints(),
            BTreeSet::from([("primary".to_owned(), "local".to_owned())])
        );
        let health = adapter.upstream_health();
        assert!(health[0].endpoints[0].drained);
        assert_eq!(adapter.pingora_package_version(), "0.9.0");
    }
}
