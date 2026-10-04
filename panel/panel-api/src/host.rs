//! The host the gateway runs on (ADR 0028).

use crate::{
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{extract::State, http::HeaderMap, Json};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{HostFilesystem, HostNetworkDevice, HostPort, HostSummary};
use panel_errors::PanelError;
use serde::Serialize;
use std::{sync::Arc, time::SystemTime};
use utoipa::ToSchema;

/// The share of a filesystem used from which it is a warning.
const DISK_WARNING: f64 = 0.85;
/// The share used from which it is critical.
const DISK_CRITICAL: f64 = 0.95;

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn HostPort>, ApiError> {
    state.host.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "the host's figures are not available here",
        ))
    })
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// How full a filesystem is.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiskLevel {
    Ok,
    /// 85% used or more.
    Warning,
    /// 95% used or more.
    Critical,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HostFilesystemView {
    pub mountpoint: String,
    pub device: String,
    pub fstype: String,
    pub size_bytes: f64,
    pub available_bytes: f64,
    /// The share used, from 0 to 1.
    pub used_ratio: f64,
    pub level: DiskLevel,
}

impl From<HostFilesystem> for HostFilesystemView {
    fn from(value: HostFilesystem) -> Self {
        let used_ratio = if value.size_bytes > 0.0 {
            (1.0 - value.available_bytes / value.size_bytes).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            level: if used_ratio >= DISK_CRITICAL {
                DiskLevel::Critical
            } else if used_ratio >= DISK_WARNING {
                DiskLevel::Warning
            } else {
                DiskLevel::Ok
            },
            used_ratio,
            mountpoint: value.mountpoint,
            device: value.device,
            fstype: value.fstype,
            size_bytes: value.size_bytes,
            available_bytes: value.available_bytes,
        }
    }
}

/// A physical network device's traffic over five minutes.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HostNetworkDeviceView {
    pub device: String,
    pub receive_bytes_per_second: f64,
    pub transmit_bytes_per_second: f64,
}

impl From<HostNetworkDevice> for HostNetworkDeviceView {
    fn from(value: HostNetworkDevice) -> Self {
        Self {
            device: value.device,
            receive_bytes_per_second: value.receive_bytes_per_second,
            transmit_bytes_per_second: value.transmit_bytes_per_second,
        }
    }
}

/// The host's figures, from the node exporter.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HostSummaryView {
    /// When the figures were read, RFC 3339.
    pub observed_at: Option<String>,
    /// False when the node exporter has no figures; the rest is then empty.
    pub reporting: bool,
    pub hostname: String,
    pub operating_system: String,
    pub kernel_release: String,
    pub architecture: String,
    /// The host's clock, RFC 3339.
    pub host_time: Option<String>,
    pub time_zone: String,
    pub uptime_seconds: Option<u64>,
    pub cpu_count: u32,
    /// The share of CPU time not idle over five minutes, from 0 to 1.
    pub cpu_usage: Option<f64>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub memory_total_bytes: f64,
    pub memory_available_bytes: f64,
    /// Fullest first.
    pub filesystems: Vec<HostFilesystemView>,
    /// Busiest first.
    pub network_devices: Vec<HostNetworkDeviceView>,
}

impl From<HostSummary> for HostSummaryView {
    fn from(value: HostSummary) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            reporting: value.reporting,
            hostname: value.hostname,
            operating_system: value.operating_system,
            kernel_release: value.kernel_release,
            architecture: value.architecture,
            host_time: value.host_time.map(rfc3339),
            time_zone: value.time_zone,
            uptime_seconds: value.uptime.map(|uptime| uptime.as_secs()),
            cpu_count: value.cpu_count,
            cpu_usage: value.cpu_usage,
            load1: value.load1,
            load5: value.load5,
            load15: value.load15,
            memory_total_bytes: value.memory_total_bytes,
            memory_available_bytes: value.memory_available_bytes,
            filesystems: value.filesystems.into_iter().map(Into::into).collect(),
            network_devices: value.network_devices.into_iter().map(Into::into).collect(),
        }
    }
}

/// The host's CPU, memory, filesystems, load, network and system.
#[utoipa::path(get, path = "/api/v1/host", params(QueryHeaders),
    responses((status = 200, body = HostSummaryView)), tag = "host")]
pub(crate) async fn host_summary<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<HostSummaryView>, ApiError> {
    let summary = port(&state)?.summary(request_scope(&headers)?).await?;
    Ok(Json(summary.into()))
}
