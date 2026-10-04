//! `HostPort` over `observability-service` (ADR 0028).

use crate::{time, ObservabilityClient};
use async_trait::async_trait;
use panel_application::{HostFilesystem, HostNetworkDevice, HostPort, HostSummary, RequestScope};
use panel_contracts::observability::v1::{self as wire, host_client::HostClient};
use panel_errors::Result;
use panel_service::{request_context, response_error, status_error};
use std::time::Duration;

fn summary(value: wire::HostSummary) -> HostSummary {
    HostSummary {
        observed_at: time(value.observed_at),
        reporting: value.reporting,
        hostname: value.hostname,
        operating_system: value.operating_system,
        kernel_release: value.kernel_release,
        architecture: value.architecture,
        host_time: time(value.host_time),
        time_zone: value.time_zone,
        uptime: value
            .uptime
            .and_then(|uptime| Duration::try_from(uptime).ok()),
        cpu_count: value.cpu_count,
        cpu_usage: value.cpu_usage,
        load1: value.load1,
        load5: value.load5,
        load15: value.load15,
        memory_total_bytes: value.memory_total_bytes,
        memory_available_bytes: value.memory_available_bytes,
        filesystems: value
            .filesystems
            .into_iter()
            .map(|filesystem| HostFilesystem {
                mountpoint: filesystem.mountpoint,
                device: filesystem.device,
                fstype: filesystem.fstype,
                size_bytes: filesystem.size_bytes,
                available_bytes: filesystem.available_bytes,
            })
            .collect(),
        network_devices: value
            .network_devices
            .into_iter()
            .map(|device| HostNetworkDevice {
                device: device.device,
                receive_bytes_per_second: device.receive_bytes_per_second,
                transmit_bytes_per_second: device.transmit_bytes_per_second,
            })
            .collect(),
    }
}

#[async_trait]
impl HostPort for ObservabilityClient {
    async fn summary(&self, scope: RequestScope) -> Result<HostSummary> {
        let message = wire::HostSummaryRequest {
            context: Some(request_context(&scope)),
        };
        let response = HostClient::new(self.channel.clone())
            .summary(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(summary(response.summary.unwrap_or_default()))
    }
}
