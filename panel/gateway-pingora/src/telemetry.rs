//! What the gateway measures (ADR 0022): the requests it serves and sends
//! upstream, its open connections and its TLS handshakes.
//!
//! A failed request's `error_type` is the status code of an error response
//! or one of `connect_timeout`, `connect_refused`, `connect_no_route`,
//! `connect_error`, `tls_handshake_failure`, `tls_handshake_timeout`,
//! `tls_certificate_invalid`, `invalid_http_header`, `http1_error`,
//! `http2_error`, `read_error`, `write_error`, `read_timeout`,
//! `write_timeout`, `connection_closed` and `_OTHER`.

use crate::{
    adapter::PingoraGatewayAdapter, certificates::TlsVersion, routing::RoutingTable,
    upstream::UpstreamPool,
};
use panel_metrics::{ErrorType, HttpClientMetrics, HttpServerMetrics, Metrics};
use pingora_core::{Error, ErrorType as Kind};
use prometheus_client::{
    collector::Collector,
    encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric},
    metrics::{
        counter::Counter,
        family::Family,
        gauge::{ConstGauge, Gauge},
        histogram::Histogram,
        TypedMetric,
    },
    registry::Unit,
};
use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

/// Upper bounds of Lua run durations, in seconds: most runs take well under
/// a millisecond.
const LUA_BUCKETS: [f64; 14] = [
    0.000_05, 0.000_1, 0.000_25, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
    1.0,
];

fn lua_histogram() -> Histogram {
    Histogram::new(LUA_BUCKETS)
}

/// The gateway's metrics, registered once and shared by every generation of
/// the data plane.
#[derive(Clone, Debug)]
pub struct GatewayMetrics {
    pub(crate) server: HttpServerMetrics,
    pub(crate) client: HttpClientMetrics,
    connections: Family<ListenerLabels, Gauge>,
    handshakes: Family<HandshakeLabels, Counter>,
    domains: Family<DomainLabels, Counter>,
    upstream_connections: Family<ConnectionLabels, Counter>,
    lua_runs: Family<LuaRunLabels, Counter>,
    lua_durations: Family<LuaPhaseLabels, Histogram, fn() -> Histogram>,
    lua_slow: Family<LuaPhaseLabels, Counter>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct LuaPhaseLabels {
    site: Option<Arc<str>>,
    route: Option<Arc<str>>,
    phase: &'static str,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct LuaRunLabels {
    site: Option<Arc<str>>,
    route: Option<Arc<str>>,
    phase: &'static str,
    /// `ok`, or how the run failed: `error`, `timeout`, `work`, `memory`
    /// or `refused`.
    outcome: &'static str,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct ListenerLabels {
    listener: Arc<str>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct ConnectionLabels {
    upstream: Arc<str>,
    /// `true` when the connection came from the pool.
    reused: &'static str,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct DomainLabels {
    site: Arc<str>,
    domain: Arc<str>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct HandshakeLabels {
    listener: Arc<str>,
    tls_protocol_version: Option<&'static str>,
}

impl GatewayMetrics {
    pub fn register(metrics: &mut Metrics) -> Self {
        let registry = metrics.registry();
        let gateway = Self {
            server: HttpServerMetrics::register(registry),
            client: HttpClientMetrics::register(registry),
            connections: Family::default(),
            handshakes: Family::default(),
            domains: Family::default(),
            upstream_connections: Family::default(),
            lua_runs: Family::default(),
            lua_durations: Family::new_with_constructor(lua_histogram),
            lua_slow: Family::default(),
        };
        registry.register(
            "pingora_panel_gateway_open_connections",
            "Number of open client connections",
            gateway.connections.clone(),
        );
        registry.register(
            "pingora_panel_gateway_tls_handshakes",
            "Number of completed TLS handshakes",
            gateway.handshakes.clone(),
        );
        registry.register(
            "pingora_panel_gateway_upstream_connections",
            "Number of connections used to reach upstreams, by whether they came from the pool",
            gateway.upstream_connections.clone(),
        );
        registry.register(
            "pingora_panel_gateway_domain_requests",
            "Number of requests by the configured domain that took them",
            gateway.domains.clone(),
        );
        registry.register(
            "pingora_panel_gateway_lua_runs",
            "Number of Lua handler runs, by phase and outcome",
            gateway.lua_runs.clone(),
        );
        registry.register_with_unit(
            "pingora_panel_gateway_lua_run_duration",
            "Duration of Lua handler runs",
            Unit::Seconds,
            gateway.lua_durations.clone(),
        );
        registry.register(
            "pingora_panel_gateway_lua_slow_runs",
            "Number of Lua handler runs longer than their slow threshold",
            gateway.lua_slow.clone(),
        );
        gateway
    }

    /// Counts and measures a Lua handler run.
    pub(crate) fn lua_run(
        &self,
        site: Option<Arc<str>>,
        route: Option<Arc<str>>,
        phase: &'static str,
        outcome: &'static str,
        duration: Duration,
        slow: bool,
    ) {
        let labels = LuaPhaseLabels { site, route, phase };
        self.lua_durations
            .get_or_create(&labels)
            .observe(duration.as_secs_f64());
        if slow {
            self.lua_slow.get_or_create(&labels).inc();
        }
        self.lua_runs
            .get_or_create(&LuaRunLabels {
                site: labels.site,
                route: labels.route,
                phase,
                outcome,
            })
            .inc();
    }

    /// The open connections of `listener`.
    pub(crate) fn connections(&self, listener: &Arc<str>) -> Gauge {
        self.connections
            .get_or_create(&ListenerLabels {
                listener: Arc::clone(listener),
            })
            .clone()
    }

    /// Counts a connection to `upstream`, new or `reused` from the pool.
    pub(crate) fn upstream_connection(&self, upstream: Arc<str>, reused: bool) {
        self.upstream_connections
            .get_or_create(&ConnectionLabels {
                upstream,
                reused: if reused { "true" } else { "false" },
            })
            .inc();
    }

    /// Counts a request that `site` took by its configured `domain`.
    pub(crate) fn domain_request(&self, site: Arc<str>, domain: Arc<str>) {
        self.domains
            .get_or_create(&DomainLabels { site, domain })
            .inc();
    }

    /// Counts a handshake completed on `listener`.
    pub(crate) fn handshake(&self, listener: &Arc<str>, version: Option<TlsVersion>) {
        self.handshakes
            .get_or_create(&HandshakeLabels {
                listener: Arc::clone(listener),
                tls_protocol_version: version.map(|version| match version {
                    TlsVersion::Tls12 => "1.2",
                    TlsVersion::Tls13 => "1.3",
                }),
            })
            .inc();
    }
}

/// Reports the active configuration at every scrape: its revision and when
/// it was activated.
pub fn register_configuration(metrics: &mut Metrics, adapter: Arc<PingoraGatewayAdapter>) {
    metrics
        .registry()
        .register_collector(Box::new(Configuration(adapter)));
}

struct Configuration(Arc<PingoraGatewayAdapter>);

impl std::fmt::Debug for Configuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Configuration").finish_non_exhaustive()
    }
}

impl Collector for Configuration {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        if let Some(revision) = self.0.active_revision() {
            let gauge = ConstGauge::new(revision.get());
            gauge.encode(encoder.encode_descriptor(
                "pingora_panel_gateway_config_revision",
                "Revision of the active configuration",
                None,
                ConstGauge::<u64>::TYPE,
            )?)?;
        }
        if let Some(activated) = self.0.activated_at() {
            let seconds = activated
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |since| since.as_secs_f64());
            let gauge = ConstGauge::new(seconds);
            gauge.encode(encoder.encode_descriptor(
                "pingora_panel_gateway_config_activated_timestamp",
                "When the active configuration was activated",
                Some(&Unit::Seconds),
                ConstGauge::<f64>::TYPE,
            )?)?;
        }
        let memory = self.0.lua_memory();
        if !memory.is_empty() {
            let mut family = encoder.encode_descriptor(
                "pingora_panel_gateway_lua_memory",
                "Memory each Lua VM of the active configuration uses",
                Some(&Unit::Bytes),
                ConstGauge::<i64>::TYPE,
            )?;
            for (vm, bytes) in memory.into_iter().enumerate() {
                let labels = [("vm", vm.to_string())];
                ConstGauge::new(i64::try_from(bytes).unwrap_or(i64::MAX))
                    .encode(family.encode_family(&labels)?)?;
            }
        }
        Ok(())
    }
}

/// The label values of a prepared snapshot's sites, routes, upstreams and
/// endpoints, made once so that requests do not allocate them.
#[derive(Debug, Default)]
pub(crate) struct SnapshotLabels {
    sites: Vec<SiteLabels>,
    domains: Vec<Arc<str>>,
    pools: Vec<PoolLabels>,
}

#[derive(Debug)]
struct SiteLabels {
    id: Arc<str>,
    routes: Vec<Arc<str>>,
}

#[derive(Debug)]
struct PoolLabels {
    id: Arc<str>,
    /// Each endpoint's address and port.
    endpoints: Vec<(Arc<str>, u16)>,
}

/// An upstream endpoint as `http.client.*` metrics name it.
pub(crate) struct EndpointLabels {
    pub upstream: Arc<str>,
    pub address: Arc<str>,
    pub port: u16,
}

impl SnapshotLabels {
    pub(crate) fn new(routing: &RoutingTable, pools: &[UpstreamPool]) -> Self {
        Self {
            sites: routing
                .sites()
                .iter()
                .map(|site| SiteLabels {
                    id: Arc::from(site.id.as_str()),
                    routes: site
                        .routes()
                        .iter()
                        .map(|route| Arc::from(route.id.as_str()))
                        .collect(),
                })
                .collect(),
            domains: routing
                .domains()
                .iter()
                .map(|domain| Arc::from(domain.as_str()))
                .collect(),
            pools: pools
                .iter()
                .map(|pool| PoolLabels {
                    id: Arc::from(pool.id.as_str()),
                    endpoints: pool
                        .endpoints
                        .iter()
                        .map(|endpoint| {
                            (
                                Arc::from(endpoint.address.ip().to_string()),
                                endpoint.address.port(),
                            )
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    pub(crate) fn site(&self, site: usize) -> Option<Arc<str>> {
        self.sites.get(site).map(|labels| Arc::clone(&labels.id))
    }

    pub(crate) fn domain(&self, domain: usize) -> Option<Arc<str>> {
        self.domains.get(domain).cloned()
    }

    pub(crate) fn route(&self, site: usize, route: usize) -> Option<Arc<str>> {
        self.sites
            .get(site)
            .and_then(|labels| labels.routes.get(route))
            .cloned()
    }

    pub(crate) fn endpoint(&self, pool: usize, endpoint: usize) -> Option<EndpointLabels> {
        let labels = self.pools.get(pool)?;
        let (address, port) = labels.endpoints.get(endpoint)?;
        Some(EndpointLabels {
            upstream: Arc::clone(&labels.id),
            address: Arc::clone(address),
            port: *port,
        })
    }
}

/// The `error.type` of a request the gateway served that ended with
/// `error`, or, without one, with a server error response.
pub(crate) fn server_error_type(error: Option<&Error>, status: Option<u16>) -> Option<ErrorType> {
    error_type(error, status, 500)
}

/// The `error.type` of a request sent upstream that ended with `error`, or,
/// without one, with an error response.
pub(crate) fn client_error_type(error: Option<&Error>, status: Option<u16>) -> Option<ErrorType> {
    error_type(error, status, 400)
}

fn error_type(error: Option<&Error>, status: Option<u16>, least: u16) -> Option<ErrorType> {
    match error {
        Some(error) => Some(kind(&error.etype)),
        None => status
            .filter(|status| *status >= least)
            .map(ErrorType::Status),
    }
}

fn kind(kind: &Kind) -> ErrorType {
    ErrorType::Named(match kind {
        Kind::HTTPStatus(status) | Kind::CustomCode(_, status) => {
            return ErrorType::Status(*status)
        }
        Kind::ConnectTimedout => "connect_timeout",
        Kind::ConnectRefused => "connect_refused",
        Kind::ConnectNoRoute => "connect_no_route",
        Kind::ConnectError
        | Kind::ConnectProxyFailure
        | Kind::BindError
        | Kind::SocketError
        | Kind::AcceptError => "connect_error",
        Kind::TLSHandshakeFailure | Kind::TLSWantX509Lookup | Kind::HandshakeError => {
            "tls_handshake_failure"
        }
        Kind::TLSHandshakeTimedout => "tls_handshake_timeout",
        Kind::InvalidCert => "tls_certificate_invalid",
        Kind::InvalidHTTPHeader => "invalid_http_header",
        Kind::H1Error => "http1_error",
        Kind::H2Error | Kind::H2Downgrade | Kind::InvalidH2 => "http2_error",
        Kind::ReadError => "read_error",
        Kind::WriteError => "write_error",
        Kind::ReadTimedout => "read_timeout",
        Kind::WriteTimedout => "write_timeout",
        Kind::ConnectionClosed => "connection_closed",
        _ => "_OTHER",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_low_cardinality_types() {
        let refused = Error::new(Kind::ConnectRefused);
        assert_eq!(
            server_error_type(Some(&*refused), Some(502)),
            Some(ErrorType::Named("connect_refused"))
        );
        let status = Error::new(Kind::HTTPStatus(413));
        assert_eq!(
            server_error_type(Some(&*status), Some(413)),
            Some(ErrorType::Status(413))
        );
        let custom = Error::new(Kind::Custom("anything"));
        assert_eq!(
            client_error_type(Some(&*custom), None),
            Some(ErrorType::Named("_OTHER"))
        );
        assert_eq!(
            server_error_type(None, Some(503)),
            Some(ErrorType::Status(503))
        );
        assert_eq!(server_error_type(None, Some(404)), None);
        assert_eq!(
            client_error_type(None, Some(404)),
            Some(ErrorType::Status(404))
        );
        assert_eq!(server_error_type(None, None), None);
    }

    #[test]
    fn handshakes_and_connections_are_counted_by_listener() {
        let mut metrics = Metrics::new();
        let gateway = GatewayMetrics::register(&mut metrics);
        let listener: Arc<str> = Arc::from("public");
        gateway.handshake(&listener, Some(TlsVersion::Tls13));
        let open = gateway.connections(&listener);
        open.inc();
        let text = metrics.encode();
        assert!(text.contains(
            "pingora_panel_gateway_tls_handshakes_total{listener=\"public\",\
             tls_protocol_version=\"1.3\"} 1"
        ));
        assert!(text.contains("pingora_panel_gateway_open_connections{listener=\"public\"} 1"));
    }
}
