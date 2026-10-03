use crate::{CommandContext, RequestScope};
use async_trait::async_trait;
use panel_domain::ContentHash;
use panel_errors::Result;
use std::time::SystemTime;

/// The serving process as it runs now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DataPlaneState {
    /// Increases with every reload; zero before the first generation.
    pub generation: u64,
    pub worker_count: u32,
    pub listeners: Vec<DataPlaneListener>,
    pub generation_started_at: Option<SystemTime>,
    /// Why the configured listeners are not all served.
    pub error: Option<String>,
    pub gateway_version: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub started_at: Option<SystemTime>,
    pub uptime_seconds: u64,
    pub observed_at: Option<SystemTime>,
    pub active_revision_id: Option<u64>,
    pub active_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DataPlaneListener {
    pub id: String,
    pub address: String,
    pub tls: bool,
    pub http1: bool,
    pub http2: bool,
}

impl DataPlaneListener {
    pub fn new(id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            address: address.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct UpstreamHealth {
    pub upstream_id: String,
    /// Whether active health checks run for this upstream.
    pub checked: bool,
    pub endpoints: Vec<EndpointHealth>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct EndpointHealth {
    pub endpoint_id: String,
    pub address: String,
    pub weight: u32,
    pub enabled: bool,
    pub backup: bool,
    pub healthy: bool,
    pub drained: bool,
    /// Set while passive health keeps the endpoint out of rotation.
    pub ejected_until: Option<SystemTime>,
    pub in_flight: u32,
    pub requests: u64,
    pub failures: u64,
    pub latency_us: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct UpstreamHealthReport {
    pub upstreams: Vec<UpstreamHealth>,
    pub observed_at: Option<SystemTime>,
    pub active_revision_id: Option<u64>,
    pub active_hash: Option<ContentHash>,
}

/// Operations on the running gateway rather than on its configuration.
#[async_trait]
pub trait GatewayRuntimePort: Send + Sync {
    async fn data_plane(&self, scope: RequestScope) -> Result<DataPlaneState>;

    /// Starts a new listener generation and drains the previous one.
    async fn reload(&self, context: CommandContext) -> Result<DataPlaneState>;

    async fn set_worker_count(
        &self,
        context: CommandContext,
        workers: u32,
    ) -> Result<DataPlaneState>;

    /// Drains in-flight requests and stops the gateway process.
    async fn shutdown(&self, context: CommandContext) -> Result<()>;

    async fn upstream_health(&self, scope: RequestScope) -> Result<UpstreamHealthReport>;

    async fn set_endpoint_drained(
        &self,
        context: CommandContext,
        upstream: String,
        endpoint: String,
        drained: bool,
    ) -> Result<UpstreamHealth>;
}
