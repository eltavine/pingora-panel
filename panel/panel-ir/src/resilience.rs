//! What upstreams do about failures and load (ADR 0038): what is retried
//! within which budget, when a circuit opens, and how many requests an
//! upstream takes at once and queues.

use serde::{Deserialize, Serialize};

/// Required by snapshots whose upstreams retry, break circuits, limit or
/// queue requests, or speak h2c.
pub const UPSTREAM_RESILIENCE_CAPABILITY: &str = "upstream.resilience";

/// A failure retried besides failed connections, which always are.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCondition {
    /// The upstream did not answer in time.
    Timeout,
    /// The connection was reset or closed before a response.
    Reset,
}

/// Retries as a share of an upstream's requests over the last ten seconds.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryBudget {
    /// Retries allowed per hundred requests.
    pub percent: u32,
    /// Retries allowed per second however few requests there are.
    #[serde(default)]
    pub min_per_second: u32,
}

/// Opens an upstream's circuit on a share of failures.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreaker {
    /// Failures per hundred requests of the last ten seconds that open it.
    pub failure_percent: u32,
    /// Requests the last ten seconds need before it may open.
    pub min_requests: u32,
    /// How long it stays open before trial requests go through.
    pub open_ms: u64,
    /// Trial requests let through; all must succeed to close it.
    pub half_open_requests: u32,
}

/// Requests over an upstream's limit wait here, first in, first out.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamQueue {
    /// Requests that may wait at once; more are answered with 503.
    pub max_waiting: u32,
    /// The longest a request waits before it is answered with 503.
    pub timeout_ms: u64,
}
