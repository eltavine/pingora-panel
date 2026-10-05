//! Upstream pools: endpoint selection, connection settings, failover and health.

use crate::resilience::{Busy, Limit, PoolState, RetryRules};
use crate::secrets::SecretSource;
use http::{header, HeaderName};
use panel_domain::{EndpointId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{
    ActiveHealthCheck, CircuitBreaker, HealthCheckProtocol, LoadBalancingPolicy,
    PassiveHealthPolicy, UpstreamPoolSpec,
};
use parking_lot::Mutex;
use pingora_core::{
    protocols::ALPN,
    upstreams::peer::{HttpPeer, PeerOptions},
    utils::tls::{parse_x509, WrappedX509},
    Error, ErrorType,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_load_balancing::{
    discovery,
    health_check::{HealthCheck, HttpHealthCheck, TcpHealthCheck},
    selection::{Consistent, Random, RoundRobin},
    Backend, Backends, LoadBalancer,
};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Attempts after a failed connection before the request fails.
const MAX_FAILOVER_ATTEMPTS: usize = 3;
const LATENCY_WEIGHT_PERCENT: u64 = 20;

/// Runtime counters and operator state of one endpoint, kept across snapshots
/// so drains and statistics survive configuration changes.
#[derive(Debug, Default)]
pub(crate) struct EndpointState {
    in_flight: AtomicU32,
    requests: AtomicU64,
    failures: AtomicU64,
    consecutive_failures: AtomicU32,
    ejected_until_ms: AtomicU64,
    drained: AtomicBool,
    latency_us: AtomicU64,
}

impl EndpointState {
    fn record_success(&self) {
        self.consecutive_failures.store(0, Relaxed);
    }

    fn record_failure(&self, policy: Option<&PassiveHealthPolicy>, now_ms: u64) {
        self.failures.fetch_add(1, Relaxed);
        let consecutive = self.consecutive_failures.fetch_add(1, Relaxed) + 1;
        if let Some(policy) = policy {
            if consecutive >= policy.failure_threshold {
                self.ejected_until_ms
                    .store(now_ms.saturating_add(policy.ejection_ms), Relaxed);
                self.consecutive_failures.store(0, Relaxed);
            }
        }
    }

    fn record_latency(&self, latency: Duration) {
        let sample = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
        let mut current = self.latency_us.load(Relaxed);
        loop {
            let next = if current == 0 {
                sample
            } else {
                let weighted = (u128::from(current) * u128::from(100 - LATENCY_WEIGHT_PERCENT)
                    + u128::from(sample) * u128::from(LATENCY_WEIGHT_PERCENT))
                    / 100;
                u64::try_from(weighted).unwrap_or(u64::MAX)
            };
            match self
                .latency_us
                .compare_exchange_weak(current, next, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

/// Endpoint state shared by every snapshot, keyed by pool and endpoint ID,
/// and each pool's own, keyed by pool ID.
#[derive(Debug, Default)]
pub(crate) struct EndpointStates {
    states: Mutex<HashMap<(String, String), Arc<EndpointState>>>,
    pools: Mutex<HashMap<String, Arc<PoolState>>>,
}

impl EndpointStates {
    fn pool(&self, pool: &str) -> Arc<PoolState> {
        Arc::clone(self.pools.lock().entry(pool.to_owned()).or_default())
    }

    fn state(&self, pool: &str, endpoint: &str) -> Arc<EndpointState> {
        Arc::clone(
            self.states
                .lock()
                .entry((pool.to_owned(), endpoint.to_owned()))
                .or_default(),
        )
    }

    /// Forgets endpoints that no longer exist in the active snapshot.
    pub(crate) fn retain(&self, live: &HashSet<(String, String)>) {
        self.states.lock().retain(|key, _| live.contains(key));
        self.pools
            .lock()
            .retain(|pool, _| live.iter().any(|(live_pool, _)| live_pool == pool));
    }

    pub(crate) fn set_drained(&self, pool: &str, endpoint: &str, drained: bool) {
        self.state(pool, endpoint).drained.store(drained, Relaxed);
    }

    pub(crate) fn drained(&self) -> BTreeSet<(String, String)> {
        self.states
            .lock()
            .iter()
            .filter(|(_, state)| state.drained.load(Relaxed))
            .map(|(key, _)| key.clone())
            .collect()
    }
}

/// Holds one in-flight request against an endpoint's connection limit.
pub(crate) struct EndpointLease {
    state: Arc<EndpointState>,
}

impl Drop for EndpointLease {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Slot(usize);

pub(crate) struct EndpointRuntime {
    pub id: EndpointId,
    pub address: SocketAddr,
    pub target: String,
    pub tls: bool,
    sni: String,
    pub weight: u32,
    pub enabled: bool,
    pub backup: bool,
    state: Arc<EndpointState>,
    backend: Option<Backend>,
}

impl EndpointRuntime {
    fn available(&self, now_ms: u64, max_connections: Option<u32>) -> bool {
        !self.state.drained.load(Relaxed)
            && self.state.ejected_until_ms.load(Relaxed) <= now_ms
            && max_connections.is_none_or(|max| self.state.in_flight.load(Relaxed) < max)
    }
}

enum Selector {
    RoundRobin(LoadBalancer<RoundRobin>),
    Random(LoadBalancer<Random>),
    Consistent(LoadBalancer<Consistent>),
}

impl Selector {
    async fn build(
        policy: &LoadBalancingPolicy,
        backends: BTreeSet<Backend>,
        check: Option<Box<dyn HealthCheck + Send + Sync>>,
    ) -> Result<Self> {
        let backends = Backends::new(discovery::Static::new(backends));
        let selector = match policy {
            LoadBalancingPolicy::RoundRobin => {
                let mut balancer = LoadBalancer::<RoundRobin>::from_backends(backends);
                if let Some(check) = check {
                    balancer.set_health_check(check);
                }
                balancer.update().await.map_err(pingora_error)?;
                Self::RoundRobin(balancer)
            }
            LoadBalancingPolicy::Random => {
                let mut balancer = LoadBalancer::<Random>::from_backends(backends);
                if let Some(check) = check {
                    balancer.set_health_check(check);
                }
                balancer.update().await.map_err(pingora_error)?;
                Self::Random(balancer)
            }
            LoadBalancingPolicy::ConsistentHash { .. } => {
                let mut balancer = LoadBalancer::<Consistent>::from_backends(backends);
                if let Some(check) = check {
                    balancer.set_health_check(check);
                }
                balancer.update().await.map_err(pingora_error)?;
                Self::Consistent(balancer)
            }
        };
        Ok(selector)
    }

    fn select_with(
        &self,
        key: &[u8],
        max_iterations: usize,
        accept: impl Fn(&Backend, bool) -> bool,
    ) -> Option<Backend> {
        match self {
            Self::RoundRobin(balancer) => balancer.select_with(key, max_iterations, accept),
            Self::Random(balancer) => balancer.select_with(key, max_iterations, accept),
            Self::Consistent(balancer) => balancer.select_with(key, max_iterations, accept),
        }
    }

    fn backends(&self) -> &Backends {
        match self {
            Self::RoundRobin(balancer) => balancer.backends(),
            Self::Random(balancer) => balancer.backends(),
            Self::Consistent(balancer) => balancer.backends(),
        }
    }
}

/// How the consistent hash key is taken from a request.
pub(crate) enum HashKey {
    None,
    ClientIp,
    Uri,
    Header(HeaderName),
    Cookie(String),
}

impl HashKey {
    fn parse(policy: &LoadBalancingPolicy) -> Result<Self> {
        let LoadBalancingPolicy::ConsistentHash { key } = policy else {
            return Ok(Self::None);
        };
        Ok(match key.split_once(':') {
            None if key == "client_ip" => Self::ClientIp,
            None if key == "uri" => Self::Uri,
            Some(("header", name)) => Self::Header(HeaderName::try_from(name).map_err(|_| {
                PanelError::validation_failed(format!("invalid hash header {name}"))
            })?),
            Some(("cookie", name)) => Self::Cookie(name.to_owned()),
            _ => {
                return Err(PanelError::validation_failed(format!(
                    "unsupported hash key {key}"
                )))
            }
        })
    }

    pub(crate) fn extract(&self, request: &RequestHeader, client: Option<SocketAddr>) -> Vec<u8> {
        match self {
            Self::None => Vec::new(),
            Self::ClientIp => client
                .map(|address| address.ip().to_string().into_bytes())
                .unwrap_or_default(),
            Self::Uri => request
                .uri
                .path_and_query()
                .map(|value| value.as_str().as_bytes().to_vec())
                .unwrap_or_default(),
            Self::Header(name) => request
                .headers
                .get(name)
                .map(|value| value.as_bytes().to_vec())
                .unwrap_or_default(),
            Self::Cookie(name) => request
                .headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(';'))
                .filter_map(|pair| pair.trim().split_once('='))
                .find(|(cookie, _)| cookie == name)
                .map(|(_, value)| value.as_bytes().to_vec())
                .unwrap_or_default(),
        }
    }
}

struct PeerSettings {
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    verify_certificate: bool,
    verify_hostname: bool,
    ca: Option<Arc<[WrappedX509]>>,
    http2: bool,
    h2c: bool,
}

impl PeerSettings {
    fn apply(&self, options: &mut PeerOptions, tls: bool) {
        options.connection_timeout = self.connect_timeout;
        options.read_timeout = self.read_timeout;
        options.write_timeout = self.write_timeout;
        options.idle_timeout = self.idle_timeout;
        if tls {
            options.verify_cert = self.verify_certificate;
            options.verify_hostname = self.verify_hostname;
            options.ca.clone_from(&self.ca);
            options.alpn = if self.http2 { ALPN::H2H1 } else { ALPN::H1 };
        } else if self.h2c {
            options.alpn = ALPN::H2;
        }
    }
}

pub(crate) struct UpstreamPool {
    pub id: UpstreamPoolId,
    pub endpoints: Vec<EndpointRuntime>,
    primary: Option<Selector>,
    backup: Option<Selector>,
    pub hash: HashKey,
    peer: PeerSettings,
    pub host_header: Option<String>,
    pub keepalive: bool,
    passive: Option<PassiveHealthPolicy>,
    pub health_interval: Option<Duration>,
    max_connections: Option<u32>,
    pub retry: RetryRules,
    breaker: Option<CircuitBreaker>,
    limit: Option<Limit>,
    state: Arc<PoolState>,
}

/// Resolves endpoint host names; IP literals are used as they are.
pub(crate) async fn resolve(host: &str, port: u16) -> Result<SocketAddr> {
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse() {
        return Ok(SocketAddr::new(ip, port));
    }
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| {
            PanelError::validation_failed(format!("cannot resolve upstream {host}: {error}"))
        })?
        .next()
        .ok_or_else(|| PanelError::validation_failed(format!("upstream {host} has no address")))
}

impl UpstreamPool {
    pub(crate) async fn compile(
        spec: &UpstreamPoolSpec,
        states: &EndpointStates,
        secrets: &dyn SecretSource,
    ) -> Result<Self> {
        let invalid = |detail: String| {
            PanelError::validation_failed(format!("upstream {}: {detail}", spec.id))
        };
        let hash = HashKey::parse(&spec.load_balancing)?;
        let ca = match &spec.tls.ca_secret_id {
            Some(secret) => Some(load_trust_anchors(&secrets.read(secret)?).map_err(invalid)?),
            None => None,
        };
        let mut endpoints = Vec::with_capacity(spec.endpoints.len());
        let mut tiers: [BTreeSet<Backend>; 2] = Default::default();
        let mut addresses = HashSet::new();
        for (index, endpoint) in spec.endpoints.iter().enumerate() {
            let address = resolve(endpoint.address.host(), endpoint.address.port()).await?;
            let host = endpoint.address.host();
            let target = if host.contains(':') {
                format!("[{host}]:{}", endpoint.address.port())
            } else {
                format!("{host}:{}", endpoint.address.port())
            };
            let sni = endpoint
                .sni
                .clone()
                .or_else(|| spec.tls.sni.clone())
                .unwrap_or_else(|| {
                    if host.parse::<std::net::IpAddr>().is_ok() {
                        String::new()
                    } else {
                        host.to_owned()
                    }
                });
            let backend = if endpoint.enabled {
                if !addresses.insert((endpoint.backup, address)) {
                    return Err(invalid(format!(
                        "endpoint {} repeats address {address}",
                        endpoint.id
                    )));
                }
                let mut backend =
                    Backend::new_with_weight(&address.to_string(), endpoint.weight as usize)
                        .map_err(pingora_error)?;
                backend.ext.insert(Slot(index));
                tiers[usize::from(endpoint.backup)].insert(backend.clone());
                Some(backend)
            } else {
                None
            };
            endpoints.push(EndpointRuntime {
                id: endpoint.id.clone(),
                address,
                target,
                tls: endpoint.address.tls(),
                sni,
                weight: endpoint.weight,
                enabled: endpoint.enabled,
                backup: endpoint.backup,
                state: states.state(spec.id.as_str(), endpoint.id.as_str()),
                backend,
            });
        }
        let peer = PeerSettings {
            connect_timeout: spec
                .connection
                .connect_timeout_ms
                .map(Duration::from_millis),
            read_timeout: spec.connection.read_timeout_ms.map(Duration::from_millis),
            write_timeout: spec.connection.write_timeout_ms.map(Duration::from_millis),
            idle_timeout: spec.connection.idle_timeout_ms.map(Duration::from_millis),
            verify_certificate: spec.tls.verify_certificate,
            verify_hostname: spec.tls.verify_hostname,
            ca,
            http2: spec.connection.http2,
            h2c: spec.connection.h2c,
        };
        let check_tls = endpoints.first().is_some_and(|endpoint| endpoint.tls);
        if spec.health_check.is_some() && endpoints.iter().any(|endpoint| endpoint.tls != check_tls)
        {
            return Err(invalid(
                "health checks need every endpoint to use the same scheme".into(),
            ));
        }
        let probe_host = spec
            .health_check
            .as_ref()
            .and_then(|check| check.host.clone())
            .or_else(|| spec.host_header.clone())
            .or_else(|| {
                spec.endpoints
                    .first()
                    .map(|endpoint| endpoint.address.host().to_owned())
            })
            .unwrap_or_default();
        let probe_sni = endpoints
            .first()
            .map(|endpoint| endpoint.sni.clone())
            .unwrap_or_default();
        let make_check = || {
            spec.health_check
                .as_ref()
                .map(|check| health_check(check, &probe_host, &probe_sni, check_tls, &peer))
                .transpose()
        };
        let [primary, backup] = tiers;
        let primary = if primary.is_empty() {
            None
        } else {
            Some(Selector::build(&spec.load_balancing, primary, make_check()?).await?)
        };
        let backup = if backup.is_empty() {
            None
        } else {
            Some(Selector::build(&spec.load_balancing, backup, make_check()?).await?)
        };
        Ok(Self {
            id: spec.id.clone(),
            endpoints,
            primary,
            backup,
            hash,
            peer,
            host_header: spec.host_header.clone(),
            keepalive: spec.connection.keepalive,
            passive: spec.passive_health.clone(),
            health_interval: spec
                .health_check
                .as_ref()
                .map(|check| Duration::from_millis(check.interval_ms)),
            max_connections: spec.connection.max_connections,
            retry: RetryRules::compile(&spec.retry_policy),
            breaker: spec.circuit_breaker,
            limit: spec
                .max_requests
                .map(|max_requests| Limit::new(max_requests, spec.queue)),
            state: states.pool(spec.id.as_str()),
        })
    }

    /// Primary endpoints first; backups only when no primary is available.
    pub(crate) fn select(&self, key: &[u8], tried: &[usize]) -> Option<usize> {
        let now = now_ms();
        let iterations = self.endpoints.len().saturating_mul(2).max(1);
        let accept = |backend: &Backend, healthy: bool| {
            backend.ext.get::<Slot>().is_some_and(|slot| {
                healthy
                    && !tried.contains(&slot.0)
                    && self.endpoints[slot.0].available(now, self.max_connections)
            })
        };
        [&self.primary, &self.backup]
            .into_iter()
            .flatten()
            .find_map(|selector| selector.select_with(key, iterations, accept))
            .and_then(|backend| backend.ext.get::<Slot>().map(|slot| slot.0))
    }

    /// Whether a failed connection is tried on another endpoint: within the
    /// retry policy when the upstream has one, otherwise up to three tries.
    pub(crate) fn may_fail_over(&self, attempts: usize, retries: u32) -> bool {
        if self.retry.attempts > 0 {
            self.may_retry(retries)
        } else {
            attempts < MAX_FAILOVER_ATTEMPTS.min(self.endpoints.len())
        }
    }

    pub(crate) fn lease(&self, endpoint: usize) -> EndpointLease {
        let state = Arc::clone(&self.endpoints[endpoint].state);
        state.in_flight.fetch_add(1, Relaxed);
        state.requests.fetch_add(1, Relaxed);
        EndpointLease { state }
    }

    /// The peer for `endpoint`; upgrades such as WebSocket always go over
    /// HTTP/1.1, which is where they exist.
    pub(crate) fn peer(&self, endpoint: usize, upgrade: bool) -> HttpPeer {
        let endpoint = &self.endpoints[endpoint];
        let mut peer = HttpPeer::new(endpoint.address, endpoint.tls, endpoint.sni.clone());
        self.peer.apply(&mut peer.options, endpoint.tls);
        if upgrade {
            peer.options.alpn = ALPN::H1;
        }
        peer
    }

    pub(crate) fn record_success(&self, endpoint: usize, trial: bool) {
        self.endpoints[endpoint].state.record_success();
        self.state
            .outcome(false, trial, self.breaker.as_ref(), now_ms());
    }

    pub(crate) fn record_failure(&self, endpoint: usize, trial: bool) {
        let now = now_ms();
        self.endpoints[endpoint]
            .state
            .record_failure(self.passive.as_ref(), now);
        self.state.outcome(true, trial, self.breaker.as_ref(), now);
    }

    /// Whether the circuit lets a request through: `Ok(true)` for a trial,
    /// or `Err` with the seconds until it may be tried again.
    pub(crate) fn admit(&self) -> std::result::Result<bool, u64> {
        self.state.admit(self.breaker.as_ref(), now_ms())
    }

    pub(crate) fn cancel_trial(&self) {
        self.state.cancel_trial();
    }

    /// A place among the requests the upstream takes at once, if it limits
    /// them; `None` when it does not.
    pub(crate) async fn place(
        &self,
    ) -> std::result::Result<Option<tokio::sync::OwnedSemaphorePermit>, Busy> {
        match &self.limit {
            Some(limit) => limit.acquire().await.map(Some),
            None => Ok(None),
        }
    }

    /// Counts a request toward the retry budget.
    pub(crate) fn count_request(&self) {
        self.state.request(now_ms());
    }

    /// Whether the policy allows retry `retries + 1`, counting it if so.
    pub(crate) fn may_retry(&self, retries: u32) -> bool {
        retries < self.retry.attempts && self.state.may_retry(self.retry.budget, now_ms())
    }

    pub(crate) fn record_latency(&self, endpoint: usize, latency: Duration) {
        self.endpoints[endpoint].state.record_latency(latency);
    }

    pub(crate) async fn run_health_checks(&self) {
        for selector in [&self.primary, &self.backup].into_iter().flatten() {
            selector.backends().run_health_check(true).await;
        }
    }

    pub(crate) fn health(&self) -> PoolHealth {
        let now = now_ms();
        PoolHealth {
            pool_id: self.id.as_str().to_owned(),
            checked: self.health_interval.is_some(),
            endpoints: self
                .endpoints
                .iter()
                .map(|endpoint| {
                    let state = &endpoint.state;
                    let selector = if endpoint.backup {
                        &self.backup
                    } else {
                        &self.primary
                    };
                    let healthy = endpoint.backend.as_ref().is_some_and(|backend| {
                        selector
                            .as_ref()
                            .is_some_and(|selector| selector.backends().ready(backend))
                    });
                    let ejected_until = state.ejected_until_ms.load(Relaxed);
                    EndpointHealth {
                        endpoint_id: endpoint.id.as_str().to_owned(),
                        address: endpoint.target.clone(),
                        weight: endpoint.weight,
                        enabled: endpoint.enabled,
                        backup: endpoint.backup,
                        healthy,
                        drained: state.drained.load(Relaxed),
                        ejected_until_ms: (ejected_until > now).then_some(ejected_until),
                        in_flight: state.in_flight.load(Relaxed),
                        requests: state.requests.load(Relaxed),
                        failures: state.failures.load(Relaxed),
                        latency_us: Some(state.latency_us.load(Relaxed)).filter(|value| *value > 0),
                    }
                })
                .collect(),
        }
    }
}

/// Live state of one upstream pool.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PoolHealth {
    pub pool_id: String,
    /// Whether active health checks run for this pool.
    pub checked: bool,
    pub endpoints: Vec<EndpointHealth>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct EndpointHealth {
    pub endpoint_id: String,
    pub address: String,
    pub weight: u32,
    pub enabled: bool,
    pub backup: bool,
    /// Active health; endpoints without checks stay healthy.
    pub healthy: bool,
    pub drained: bool,
    /// Set while passive health keeps the endpoint out of rotation.
    pub ejected_until_ms: Option<u64>,
    pub in_flight: u32,
    pub requests: u64,
    pub failures: u64,
    /// Smoothed time to the upstream response header.
    pub latency_us: Option<u64>,
}

fn health_check(
    check: &ActiveHealthCheck,
    host: &str,
    sni: &str,
    tls: bool,
    peer: &PeerSettings,
) -> Result<Box<dyn HealthCheck + Send + Sync>> {
    let timeout = Some(Duration::from_millis(check.timeout_ms));
    let success = check.healthy_threshold as usize;
    let failure = check.unhealthy_threshold as usize;
    Ok(match check.protocol {
        HealthCheckProtocol::Tcp => {
            let mut probe = if tls {
                TcpHealthCheck::new_tls(sni)
            } else {
                TcpHealthCheck::new()
            };
            probe.consecutive_success = success;
            probe.consecutive_failure = failure;
            probe.peer_template.options.connection_timeout = timeout;
            probe
        }
        HealthCheckProtocol::Http => {
            let mut probe = HttpHealthCheck::new(host, tls);
            probe.consecutive_success = success;
            probe.consecutive_failure = failure;
            probe.peer_template.sni = sni.to_owned();
            peer.apply(&mut probe.peer_template.options, tls);
            probe.peer_template.options.connection_timeout = timeout;
            probe.peer_template.options.read_timeout = timeout;
            let mut request =
                RequestHeader::build(check.method.as_str(), check.path.as_bytes(), None)
                    .map_err(pingora_error)?;
            request
                .insert_header(header::HOST, host)
                .map_err(pingora_error)?;
            probe.req = request;
            let expected = check.expected_statuses.clone();
            probe.validator = Some(Box::new(move |response: &ResponseHeader| {
                let status = response.status.as_u16();
                let healthy = if expected.is_empty() {
                    (200..400).contains(&status)
                } else {
                    expected.contains(&status)
                };
                if healthy {
                    Ok(())
                } else {
                    Error::e_explain(
                        ErrorType::CustomCode("unexpected health check status", status),
                        format!("health check answered {status}"),
                    )
                }
            }));
            Box::new(probe)
        }
    })
}

fn load_trust_anchors(pem: &[u8]) -> std::result::Result<Arc<[WrappedX509]>, String> {
    let certificates = CertificateDer::pem_slice_iter(pem)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| format!("CA bundle is not PEM: {error}"))?;
    if certificates.is_empty() {
        return Err("CA bundle contains no certificate".into());
    }
    let mut store = rustls::RootCertStore::empty();
    for certificate in &certificates {
        store
            .add(certificate.clone())
            .map_err(|error| format!("CA certificate is invalid: {error}"))?;
    }
    Ok(certificates
        .into_iter()
        .map(|certificate| WrappedX509::new(certificate.to_vec(), parse_x509))
        .collect::<Vec<_>>()
        .into())
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn pingora_error(error: Box<Error>) -> PanelError {
    PanelError::internal(format!("Pingora upstream setup failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::NoSecrets;
    use panel_domain::EndpointAddress;
    use panel_ir::UpstreamEndpoint;

    fn endpoint(id: &str, port: u16) -> UpstreamEndpoint {
        UpstreamEndpoint::new(
            EndpointId::new(id).unwrap(),
            EndpointAddress::new("127.0.0.1", port, false).unwrap(),
        )
    }

    async fn pool(spec: &UpstreamPoolSpec, states: &EndpointStates) -> UpstreamPool {
        UpstreamPool::compile(spec, states, &NoSecrets)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn weighted_round_robin_spreads_by_weight() {
        let mut heavy = endpoint("heavy", 1001);
        heavy.weight = 3;
        let spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![heavy, endpoint("light", 1002)],
        );
        let pool = pool(&spec, &EndpointStates::default()).await;
        let mut counts = [0; 2];
        for _ in 0..400 {
            counts[pool.select(b"", &[]).unwrap()] += 1;
        }
        assert_eq!(counts, [300, 100]);
    }

    #[tokio::test]
    async fn backups_serve_only_when_primaries_are_unavailable() {
        let mut backup = endpoint("backup", 1002);
        backup.backup = true;
        let spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![endpoint("primary", 1001), backup],
        );
        let states = EndpointStates::default();
        let pool = pool(&spec, &states).await;
        assert_eq!(pool.select(b"", &[]), Some(0));
        assert_eq!(pool.select(b"", &[0]), Some(1));
        states.set_drained("pool", "primary", true);
        assert_eq!(pool.select(b"", &[]), Some(1));
        states.set_drained("pool", "backup", true);
        assert_eq!(pool.select(b"", &[]), None);
        assert_eq!(states.drained().len(), 2);
    }

    #[tokio::test]
    async fn passive_health_ejects_after_consecutive_failures() {
        let mut spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![endpoint("only", 1001)],
        );
        spec.passive_health = Some(PassiveHealthPolicy {
            failure_threshold: 2,
            ejection_ms: 60_000,
        });
        let pool = pool(&spec, &EndpointStates::default()).await;
        pool.record_failure(0, false);
        assert_eq!(pool.select(b"", &[]), Some(0));
        pool.record_failure(0, false);
        assert_eq!(pool.select(b"", &[]), None);
        let health = pool.health();
        assert!(health.endpoints[0].ejected_until_ms.is_some());
        assert_eq!(health.endpoints[0].failures, 2);
    }

    #[tokio::test]
    async fn connection_limits_and_disabled_endpoints_are_respected() {
        let mut disabled = endpoint("disabled", 1002);
        disabled.enabled = false;
        let mut spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![endpoint("limited", 1001), disabled],
        );
        spec.connection.max_connections = Some(1);
        let pool = pool(&spec, &EndpointStates::default()).await;
        let lease = pool.lease(0);
        assert_eq!(pool.select(b"", &[]), None);
        drop(lease);
        assert_eq!(pool.select(b"", &[]), Some(0));
        assert!(!pool.health().endpoints[1].enabled);
    }

    #[tokio::test]
    async fn consistent_hash_keys_are_stable() {
        let mut spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            (0..4)
                .map(|index| endpoint(&format!("node-{index}"), 1001 + index))
                .collect(),
        );
        spec.load_balancing = LoadBalancingPolicy::ConsistentHash {
            key: "cookie:session".into(),
        };
        let pool = pool(&spec, &EndpointStates::default()).await;
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request
            .insert_header(header::COOKIE, "theme=dark; session=abc123")
            .unwrap();
        let key = pool.hash.extract(&request, None);
        assert_eq!(key, b"abc123");
        let first = pool.select(&key, &[]);
        assert!((0..32).all(|_| pool.select(&key, &[]) == first));
    }

    #[tokio::test]
    async fn duplicate_addresses_and_bad_ca_bundles_are_rejected() {
        let spec = UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![endpoint("a", 1001), endpoint("b", 1001)],
        );
        assert!(
            UpstreamPool::compile(&spec, &EndpointStates::default(), &NoSecrets)
                .await
                .is_err()
        );
        assert!(load_trust_anchors(b"not pem").is_err());
    }
}
