//! What the gateway served, as the API reads it (ADR 0022). Counts are
//! computed from metrics, so they are estimates and may be fractional.

use crate::RequestScope;
use async_trait::async_trait;
use panel_domain::{RouteId, SiteId};
use panel_errors::Result;
use std::time::{Duration, SystemTime};

/// Which requests to read, over which window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrafficQuery {
    /// Every site when unset.
    pub site: Option<SiteId>,
    /// Every route of the site when unset; a route needs its site.
    pub route: Option<RouteId>,
    /// An hour when unset; the source bounds it.
    pub window: Option<Duration>,
}

/// Requests by the class of their status code.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusClasses {
    pub informational: f64,
    pub success: f64,
    pub redirection: f64,
    pub client_error: f64,
    pub server_error: f64,
}

/// Latency quantiles, unset when no request was measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Latency {
    pub p50: Option<Duration>,
    pub p90: Option<Duration>,
    pub p95: Option<Duration>,
    pub p99: Option<Duration>,
}

/// Requests the gateway sent to one upstream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpstreamTraffic {
    pub upstream: String,
    pub requests: f64,
    /// The share of attempts that failed, from 0 to 1.
    pub error_ratio: f64,
    pub latency: Latency,
}

/// Requests one route served.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RouteTraffic {
    pub site: String,
    pub route: String,
    pub requests: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrafficSummary {
    pub observed_at: Option<SystemTime>,
    pub window: Duration,
    pub requests: f64,
    pub requests_per_second: f64,
    pub statuses: StatusClasses,
    pub latency: Latency,
    /// Request body bytes received.
    pub bytes_received: f64,
    /// Response body bytes sent.
    pub bytes_sent: f64,
    pub open_connections: f64,
    pub tls_handshakes: f64,
    /// Busiest first.
    pub upstreams: Vec<UpstreamTraffic>,
    /// Busiest first.
    pub routes: Vec<RouteTraffic>,
    /// The revision of the gateway's active configuration.
    pub revision: Option<u64>,
    pub activated_at: Option<SystemTime>,
}

/// The traffic at one moment, averaged over one step.
#[derive(Clone, Debug, PartialEq)]
pub struct TrafficPoint {
    pub at: SystemTime,
    pub requests_per_second: f64,
    pub server_errors_per_second: f64,
    pub p95: Option<Duration>,
}

/// Reads what the gateway served.
#[async_trait]
pub trait TrafficPort: Send + Sync {
    async fn summary(&self, scope: RequestScope, query: TrafficQuery) -> Result<TrafficSummary>;

    /// Points oldest first, `step` apart; the source picks the step when it
    /// is unset and bounds the number of points.
    async fn series(
        &self,
        scope: RequestScope,
        query: TrafficQuery,
        step: Option<Duration>,
    ) -> Result<Vec<TrafficPoint>>;
}
