//! Data plane lifecycle: fixed listener sets per generation, graceful reload
//! and drain.
//!
//! The gateway owns every listening socket. Each generation serves duplicates
//! of them, so a reload hands the same sockets — and their accept queues — to
//! the next generation while the previous one finishes in-flight requests.

use crate::{
    acme::ChallengeDirectory,
    adapter::{ActiveSnapshot, PingoraGatewayAdapter},
    certificates::{HandshakeRecorder, ListenerCertificates},
    head_deadline::{Connections, HeadDeadline},
    listeners::{self, ListenerPlan, SocketKey},
    proxy::{ListenerContext, PanelProxy},
    telemetry::GatewayMetrics,
};
use panel_errors::{PanelError, Result};
use pingora_core::{
    apps::HttpServerOptions,
    listeners::tls::TlsSettings,
    server::configuration::ServerConf,
    services::{listening::Service, Service as _},
};
use pingora_proxy::HttpProxy;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    future::Future,
    io,
    net::TcpListener,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{
    runtime::Runtime,
    sync::{watch, Mutex},
    task::JoinHandle,
};

const DRAIN_POLL: Duration = Duration::from_millis(50);
const HEALTH_TICK: Duration = Duration::from_millis(100);
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_UPSTREAM_POOL_SIZE: usize = 128;

#[derive(Clone, Debug)]
pub struct DataPlaneOptions {
    workers: NonZeroUsize,
    drain_timeout: Duration,
    upstream_pool_size: usize,
    metrics: Option<GatewayMetrics>,
}

impl DataPlaneOptions {
    pub fn new(workers: NonZeroUsize) -> Self {
        Self {
            workers,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
            upstream_pool_size: DEFAULT_UPSTREAM_POOL_SIZE,
            metrics: None,
        }
    }

    /// Records requests, connections and handshakes in `metrics`.
    pub fn with_metrics(mut self, metrics: GatewayMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// How long a replaced generation may finish in-flight requests.
    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = timeout;
        self
    }

    /// Idle upstream connections kept for reuse per generation.
    pub fn with_upstream_pool_size(mut self, size: usize) -> Self {
        self.upstream_pool_size = size;
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DataPlaneStatus {
    /// Increments with every reload; zero before the first generation.
    pub generation: u64,
    pub workers: usize,
    pub listeners: Vec<ListenerStatus>,
    pub started_at: Option<SystemTime>,
    /// Why the active snapshot's listeners are not all served.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ListenerStatus {
    pub id: String,
    pub address: String,
    pub tls: bool,
    pub http1: bool,
    pub http2: bool,
}

pub struct DataPlane {
    adapter: Arc<PingoraGatewayAdapter>,
    options: DataPlaneOptions,
    workers: AtomicUsize,
    state: Mutex<State>,
    status: watch::Sender<DataPlaneStatus>,
}

#[derive(Default)]
struct State {
    sockets: BTreeMap<SocketKey, TcpListener>,
    current: Option<Generation>,
    retiring: Vec<JoinHandle<()>>,
    generations: u64,
}

struct Generation {
    id: u64,
    plans: Vec<ListenerPlan>,
    workers: usize,
    runtime: Option<Runtime>,
    shutdown: watch::Sender<bool>,
    in_flight: Arc<AtomicUsize>,
    started_at: SystemTime,
}

impl DataPlane {
    pub fn new(adapter: Arc<PingoraGatewayAdapter>, options: DataPlaneOptions) -> Arc<Self> {
        let workers = options.workers.get();
        Arc::new(Self {
            adapter,
            options,
            workers: AtomicUsize::new(workers),
            state: Mutex::new(State::default()),
            status: watch::channel(DataPlaneStatus {
                workers,
                ..DataPlaneStatus::default()
            })
            .0,
        })
    }

    /// Serves the active snapshot's listeners and follows every activation
    /// until `shutdown` completes, then drains.
    pub async fn run(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send) {
        let mut activations = self.adapter.subscribe();
        let health = tokio::spawn(Arc::clone(&self).run_health_checks());
        tokio::pin!(shutdown);
        loop {
            let failed = match self.reconcile(false).await {
                Ok(()) => false,
                Err(error) => {
                    tracing::error!(event = "data_plane_reconcile_failed", error = %error);
                    self.status
                        .send_modify(|status| status.error = Some(error.message.clone()));
                    true
                }
            };
            tokio::select! {
                changed = activations.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                () = tokio::time::sleep(RETRY_INTERVAL), if failed => {}
                () = &mut shutdown => break,
            }
        }
        health.abort();
        self.stop().await;
    }

    /// Starts a new generation even if nothing changed and drains the old one.
    pub async fn reload(&self) -> Result<DataPlaneStatus> {
        self.reconcile(true).await?;
        Ok(self.status())
    }

    /// Applies a new worker count through a reload.
    pub async fn set_workers(&self, workers: NonZeroUsize) -> Result<DataPlaneStatus> {
        let previous = self.workers.swap(workers.get(), Relaxed);
        if let Err(error) = self.reconcile(false).await {
            self.workers.store(previous, Relaxed);
            return Err(error);
        }
        self.status
            .send_modify(|status| status.workers = workers.get());
        Ok(self.status())
    }

    pub fn status(&self) -> DataPlaneStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<DataPlaneStatus> {
        self.status.subscribe()
    }

    async fn reconcile(&self, force: bool) -> Result<()> {
        let plans = self
            .adapter
            .active_prepared()
            .map(|prepared| prepared.listeners.clone())
            .unwrap_or_default();
        let workers = self.workers.load(Relaxed);
        let mut state = self.state.lock().await;
        let unchanged = match &state.current {
            Some(current) => current.plans == plans && current.workers == workers,
            None => plans.is_empty(),
        };
        if unchanged && !force {
            if self.status.borrow().error.is_some() {
                self.status.send_modify(|status| status.error = None);
            }
            return Ok(());
        }
        let desired: BTreeSet<SocketKey> = plans.iter().map(|plan| plan.socket.clone()).collect();
        let mut fresh = BTreeMap::new();
        let mut hard_switch = false;
        for key in desired
            .iter()
            .filter(|key| !state.sockets.contains_key(*key))
        {
            match listeners::bind(key) {
                Ok(socket) => {
                    fresh.insert(key.clone(), socket);
                }
                Err(error)
                    if error.kind() == io::ErrorKind::AddrInUse
                        && state.sockets.keys().any(|held| {
                            held.address.port() == key.address.port() && !desired.contains(held)
                        }) =>
                {
                    hard_switch = true;
                }
                Err(error) => return Err(bind_error(key, &error)),
            }
        }
        if hard_switch {
            // A socket being removed occupies the port, so it must close first.
            drop(fresh);
            fresh = BTreeMap::new();
            if let Some(previous) = state.current.take() {
                previous.retire(self.options.drain_timeout).await;
            }
            state.sockets.retain(|key, _| desired.contains(key));
            for key in desired
                .iter()
                .filter(|key| !state.sockets.contains_key(*key))
            {
                let socket = listeners::bind(key).map_err(|error| bind_error(key, &error))?;
                fresh.insert(key.clone(), socket);
            }
        }
        state.sockets.extend(fresh);
        let generation = if plans.is_empty() {
            None
        } else {
            let id = state.generations + 1;
            let generation = Generation::start(
                id,
                plans,
                workers,
                &state.sockets,
                self.adapter.active(),
                self.adapter.challenges(),
                &self.options,
            )?;
            state.generations = id;
            Some(generation)
        };
        let previous = std::mem::replace(&mut state.current, generation);
        state.sockets.retain(|key, _| desired.contains(key));
        self.adapter
            .set_bound_sockets(state.sockets.keys().cloned().collect());
        state.retiring.retain(|task| !task.is_finished());
        if let Some(previous) = previous {
            let drain = self.options.drain_timeout;
            state.retiring.push(tokio::spawn(previous.retire(drain)));
        }
        let status = DataPlaneStatus {
            generation: state.generations,
            workers,
            listeners: state
                .current
                .as_ref()
                .map(|current| current.plans.iter().map(ListenerStatus::from).collect())
                .unwrap_or_default(),
            started_at: state.current.as_ref().map(|current| current.started_at),
            error: None,
        };
        if let Some(current) = &state.current {
            tracing::info!(
                event = "data_plane_generation_started",
                generation = current.id,
                workers,
                listeners = current.plans.len()
            );
        }
        self.status.send_replace(status);
        Ok(())
    }

    async fn stop(&self) {
        let mut state = self.state.lock().await;
        let current = state.current.take();
        let retiring = std::mem::take(&mut state.retiring);
        state.sockets.clear();
        drop(state);
        if let Some(current) = current {
            current.retire(self.options.drain_timeout).await;
        }
        for task in retiring {
            let _ = task.await;
        }
        self.adapter.set_bound_sockets(BTreeSet::new());
        self.status.send_modify(|status| {
            status.listeners.clear();
            status.started_at = None;
        });
    }

    /// Runs each pool's active checks at its interval without overlapping runs.
    async fn run_health_checks(self: Arc<Self>) {
        let running = Arc::new(parking_lot::Mutex::new(HashSet::<String>::new()));
        let mut due: HashMap<String, Instant> = HashMap::new();
        let mut tick = tokio::time::interval(HEALTH_TICK);
        loop {
            tick.tick().await;
            let Some(prepared) = self.adapter.active_prepared() else {
                continue;
            };
            let now = Instant::now();
            for (index, pool) in prepared.pools.iter().enumerate() {
                let Some(interval) = pool.health_interval else {
                    continue;
                };
                let id = pool.id.as_str().to_owned();
                let next = due.entry(id.clone()).or_insert(now);
                if now < *next || !running.lock().insert(id.clone()) {
                    continue;
                }
                *next = now + interval;
                let prepared = Arc::clone(&prepared);
                let running = Arc::clone(&running);
                tokio::spawn(async move {
                    prepared.pools[index].run_health_checks().await;
                    running.lock().remove(&id);
                });
            }
        }
    }
}

impl Generation {
    fn start(
        id: u64,
        plans: Vec<ListenerPlan>,
        workers: usize,
        sockets: &BTreeMap<SocketKey, TcpListener>,
        active: ActiveSnapshot,
        challenges: Option<Arc<ChallengeDirectory>>,
        options: &DataPlaneOptions,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name(format!("pingora-g{id}"))
            .enable_all()
            .build()
            .map_err(|error| {
                PanelError::unavailable(format!("cannot start data plane workers: {error}"))
            })?;
        let conf = Arc::new(ServerConf {
            threads: workers,
            upstream_keepalive_pool_size: options.upstream_pool_size,
            ..ServerConf::default()
        });
        let (shutdown, watch) = watch::channel(false);
        let in_flight = Arc::new(AtomicUsize::new(0));
        for plan in &plans {
            let socket = sockets.get(&plan.socket).ok_or_else(|| {
                PanelError::internal(format!("listener {} has no socket", plan.id))
            })?;
            let label: Arc<str> = Arc::from(plan.id.as_str());
            let metrics = options.metrics.clone();
            let connections = Arc::new(Connections::counted(
                metrics.as_ref().map(|metrics| metrics.connections(&label)),
            ));
            let handshakes = HandshakeRecorder(metrics.clone().map(|metrics| (metrics, label)));
            let proxy = PanelProxy::new(
                ListenerContext {
                    id: plan.id.clone(),
                    metrics,
                    tls: plan.tls,
                    http1: plan.http1,
                    challenges: challenges.clone(),
                    client: plan.client.clone(),
                    connections: Arc::clone(&connections),
                },
                Arc::clone(&active),
                Arc::clone(&in_flight),
            );
            let mut server_options = HttpServerOptions::default();
            server_options.h2c = plan.http2 && !plan.tls;
            let mut proxy = HttpProxy::new(proxy, Arc::clone(&conf));
            proxy.server_options = Some(server_options);
            proxy.handle_init_modules();
            let mut service = Service::new(
                format!("listener {}", plan.id),
                HeadDeadline::new(proxy, plan.head_timeout, connections),
            );
            let address = plan.socket.address.to_string();
            if plan.tls {
                let config = plan.server_config(Arc::new(ListenerCertificates::new(
                    Arc::clone(&active),
                    plan.id.clone(),
                )))?;
                let settings = TlsSettings::from_server_config(config, Some(Box::new(handshakes)));
                service.add_tls_with_settings(&address, None, settings);
            } else {
                service.add_tcp(&address);
            }
            let fds = inherited(&address, socket)?;
            let shutdown = watch.clone();
            runtime.spawn(async move {
                #[cfg(unix)]
                service.start_service(fds, shutdown, 1).await;
                #[cfg(not(unix))]
                {
                    let () = fds;
                    service.start_service(shutdown, 1).await;
                }
            });
        }
        Ok(Self {
            id,
            plans,
            workers,
            runtime: Some(runtime),
            shutdown,
            in_flight,
            started_at: SystemTime::now(),
        })
    }

    /// Stops accepting, lets in-flight requests finish within `drain`, then
    /// closes what remains, such as idle keep-alive connections.
    async fn retire(mut self, drain: Duration) {
        let _ = self.shutdown.send(true);
        let deadline = Instant::now() + drain;
        while self.in_flight.load(Relaxed) > 0 && Instant::now() < deadline {
            tokio::time::sleep(DRAIN_POLL).await;
        }
        tracing::info!(
            event = "data_plane_generation_retired",
            generation = self.id,
            abandoned_requests = self.in_flight.load(Relaxed)
        );
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// A descriptor table holding a duplicate of the master socket, which the
/// generation owns and closes when it stops.
#[cfg(unix)]
fn inherited(
    address: &str,
    socket: &TcpListener,
) -> Result<Option<pingora_core::server::ListenFds>> {
    use std::os::fd::IntoRawFd;
    let duplicate = socket
        .try_clone()
        .map_err(|error| PanelError::unavailable(format!("cannot share {address}: {error}")))?;
    let mut table = pingora_core::server::Fds::new();
    table.add(address.to_owned(), duplicate.into_raw_fd());
    Ok(Some(Arc::new(parking_lot::Mutex::new(table))))
}

#[cfg(not(unix))]
fn inherited(_address: &str, _socket: &TcpListener) -> Result<()> {
    Ok(())
}

fn bind_error(key: &SocketKey, error: &io::Error) -> PanelError {
    PanelError::unavailable(format!("cannot listen on {}: {error}", key.address))
}

impl From<&ListenerPlan> for ListenerStatus {
    fn from(plan: &ListenerPlan) -> Self {
        Self {
            id: plan.id.clone(),
            address: plan.socket.address.to_string(),
            tls: plan.tls,
            http1: plan.http1,
            http2: plan.http2,
        }
    }
}
