//! The host the gateway runs on, as the node exporter reports it
//! (ADR 0028).

use crate::RequestScope;
use async_trait::async_trait;
use panel_errors::Result;
use std::time::{Duration, SystemTime};

/// A mounted filesystem that holds data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostFilesystem {
    pub mountpoint: String,
    pub device: String,
    pub fstype: String,
    pub size_bytes: f64,
    pub available_bytes: f64,
}

/// A physical network device's traffic over five minutes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostNetworkDevice {
    pub device: String,
    pub receive_bytes_per_second: f64,
    pub transmit_bytes_per_second: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostSummary {
    pub observed_at: Option<SystemTime>,
    /// False when the node exporter has no figures; the rest is then unset.
    pub reporting: bool,
    pub hostname: String,
    pub operating_system: String,
    pub kernel_release: String,
    pub architecture: String,
    pub host_time: Option<SystemTime>,
    pub time_zone: String,
    pub uptime: Option<Duration>,
    pub cpu_count: u32,
    /// The share of CPU time not idle, from 0 to 1.
    pub cpu_usage: Option<f64>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub memory_total_bytes: f64,
    pub memory_available_bytes: f64,
    /// Fullest first.
    pub filesystems: Vec<HostFilesystem>,
    /// Busiest first.
    pub network_devices: Vec<HostNetworkDevice>,
}

#[async_trait]
pub trait HostPort: Send + Sync {
    async fn summary(&self, scope: RequestScope) -> Result<HostSummary>;
}
