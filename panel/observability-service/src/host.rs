//! `pingora.panel.observability.v1.Host` over the node exporter's metrics in
//! Prometheus (ADR 0028), with fixed queries.

use crate::promql::{self, Values};
use panel_contracts::observability::v1::{self as wire, host_server::Host};
use panel_errors::Result;
use prometheus_http_query::Client;
use std::{
    collections::{BTreeMap, HashMap},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::{Request, Response, Status};

const NODE: &str = "job=\"node\"";
/// Filesystems that hold data, not virtual ones or container layers.
const REAL_FILESYSTEMS: &str =
    "fstype!~\"tmpfs|ramfs|devtmpfs|overlay|squashfs|nsfs|tracefs|autofs|proc|sysfs|fuse\\\\..*\"";
/// Physical network devices, not loopback, bridges or container links.
const PHYSICAL_DEVICES: &str =
    "device!~\"lo|veth.*|docker.*|br-.*|cni.*|flannel.*|virbr.*|tun.*|tap.*|cali.*|vxlan.*\"";

fn timestamp(seconds: f64) -> Option<prost_types::Timestamp> {
    (seconds.is_finite() && seconds >= 0.0)
        .then(|| UNIX_EPOCH.checked_add(Duration::from_secs_f64(seconds)))
        .flatten()
        .map(Into::into)
}

fn label(values: &Values, name: &str) -> String {
    values
        .first()
        .and_then(|(labels, _)| labels.get(name))
        .cloned()
        .unwrap_or_default()
}

/// Values by a label set's entries, such as each device's bytes.
fn by(values: Values, names: &[&str]) -> BTreeMap<Vec<String>, f64> {
    values
        .into_iter()
        .filter(|(_, value)| value.is_finite())
        .map(|(labels, value)| {
            let key = names
                .iter()
                .map(|name| labels.get(*name).cloned().unwrap_or_default())
                .collect();
            (key, value)
        })
        .collect()
}

fn filesystems(sizes: Values, available: Values) -> Vec<wire::HostFilesystem> {
    let names = ["mountpoint", "device", "fstype"];
    let available = by(available, &names);
    // A device mounted twice, as with bind mounts, counts once, by its
    // shortest mount point.
    let mut devices: HashMap<String, wire::HostFilesystem> = HashMap::new();
    for (key, size) in by(sizes, &names) {
        let Some(free) = available.get(&key) else {
            continue;
        };
        let [mountpoint, device, fstype] = <[String; 3]>::try_from(key).unwrap_or_default();
        if size <= 0.0 {
            continue;
        }
        let filesystem = wire::HostFilesystem {
            mountpoint,
            device: device.clone(),
            fstype,
            size_bytes: size,
            available_bytes: *free,
        };
        match devices.get(&device) {
            Some(kept) if kept.mountpoint.len() <= filesystem.mountpoint.len() => {}
            _ => {
                devices.insert(device, filesystem);
            }
        }
    }
    let used = |filesystem: &wire::HostFilesystem| {
        1.0 - filesystem.available_bytes / filesystem.size_bytes
    };
    let mut filesystems: Vec<_> = devices.into_values().collect();
    filesystems.sort_by(|left, right| {
        used(right)
            .total_cmp(&used(left))
            .then_with(|| left.mountpoint.cmp(&right.mountpoint))
    });
    filesystems
}

fn devices(receive: Values, transmit: Values) -> Vec<wire::HostNetworkDevice> {
    let transmit = by(transmit, &["device"]);
    let mut devices: Vec<_> = by(receive, &["device"])
        .into_iter()
        .map(|(key, received)| wire::HostNetworkDevice {
            transmit_bytes_per_second: transmit.get(&key).copied().unwrap_or_default(),
            device: key.into_iter().next().unwrap_or_default(),
            receive_bytes_per_second: received,
        })
        .collect();
    let total = |device: &wire::HostNetworkDevice| {
        device.receive_bytes_per_second + device.transmit_bytes_per_second
    };
    devices.sort_by(|left, right| total(right).total_cmp(&total(left)));
    devices
}

pub struct HostService {
    prometheus: Client,
}

impl HostService {
    pub fn new(prometheus: Client) -> Self {
        Self { prometheus }
    }

    async fn instant(&self, query: String) -> Result<Values> {
        promql::instant(&self.prometheus, &query).await
    }

    async fn value(&self, query: String) -> Result<Option<f64>> {
        Ok(self
            .instant(query)
            .await?
            .first()
            .map(|(_, value)| *value)
            .filter(|value| value.is_finite()))
    }

    pub async fn summarize(&self) -> Result<wire::HostSummary> {
        let (uname, os, zone, time, boot, cpus, cpu) = tokio::try_join!(
            self.instant(format!("node_uname_info{{{NODE}}}")),
            self.instant(format!("node_os_info{{{NODE}}}")),
            self.instant(format!("node_time_zone_offset_seconds{{{NODE}}}")),
            self.value(format!("node_time_seconds{{{NODE}}}")),
            self.value(format!("node_boot_time_seconds{{{NODE}}}")),
            self.value(format!(
                "count(node_cpu_seconds_total{{{NODE},mode=\"idle\"}})"
            )),
            self.value(format!(
                "1 - avg(rate(node_cpu_seconds_total{{{NODE},mode=\"idle\"}}[5m]))"
            )),
        )?;
        let (load1, load5, load15, memory_total, memory_available) = tokio::try_join!(
            self.value(format!("node_load1{{{NODE}}}")),
            self.value(format!("node_load5{{{NODE}}}")),
            self.value(format!("node_load15{{{NODE}}}")),
            self.value(format!("node_memory_MemTotal_bytes{{{NODE}}}")),
            self.value(format!("node_memory_MemAvailable_bytes{{{NODE}}}")),
        )?;
        let (sizes, available, receive, transmit) = tokio::try_join!(
            self.instant(format!(
                "node_filesystem_size_bytes{{{NODE},{REAL_FILESYSTEMS}}}"
            )),
            self.instant(format!(
                "node_filesystem_avail_bytes{{{NODE},{REAL_FILESYSTEMS}}}"
            )),
            self.instant(format!(
                "rate(node_network_receive_bytes_total{{{NODE},{PHYSICAL_DEVICES}}}[5m])"
            )),
            self.instant(format!(
                "rate(node_network_transmit_bytes_total{{{NODE},{PHYSICAL_DEVICES}}}[5m])"
            )),
        )?;
        let observed_at = Some(SystemTime::now().into());
        if uname.is_empty() {
            return Ok(wire::HostSummary {
                observed_at,
                reporting: false,
                ..wire::HostSummary::default()
            });
        }
        let pretty = label(&os, "pretty_name");
        let operating_system = if pretty.is_empty() {
            format!("{} {}", label(&os, "name"), label(&os, "version"))
                .trim()
                .to_owned()
        } else {
            pretty
        };
        Ok(wire::HostSummary {
            observed_at,
            reporting: true,
            hostname: label(&uname, "nodename"),
            operating_system,
            kernel_release: label(&uname, "release"),
            architecture: label(&uname, "machine"),
            host_time: time.and_then(timestamp),
            time_zone: label(&zone, "time_zone"),
            uptime: time
                .zip(boot)
                .map(|(time, boot)| time - boot)
                .filter(|uptime| uptime.is_finite() && *uptime >= 0.0)
                .and_then(|uptime| {
                    prost_types::Duration::try_from(Duration::from_secs_f64(uptime)).ok()
                }),
            cpu_count: cpus.map_or(0, |count| count as u32),
            cpu_usage: cpu.map(|usage| usage.clamp(0.0, 1.0)),
            load1: load1.unwrap_or_default(),
            load5: load5.unwrap_or_default(),
            load15: load15.unwrap_or_default(),
            memory_total_bytes: memory_total.unwrap_or_default(),
            memory_available_bytes: memory_available.unwrap_or_default(),
            filesystems: filesystems(sizes, available),
            network_devices: devices(receive, transmit),
        })
    }
}

#[tonic::async_trait]
impl Host for HostService {
    async fn summary(
        &self,
        _request: Request<wire::HostSummaryRequest>,
    ) -> std::result::Result<Response<wire::HostSummaryResponse>, Status> {
        Ok(Response::new(match self.summarize().await {
            Ok(summary) => wire::HostSummaryResponse {
                summary: Some(summary),
                error: None,
            },
            Err(error) => wire::HostSummaryResponse {
                summary: None,
                error: Some(error.into()),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(labels: &[(&str, &str)], value: f64) -> (HashMap<String, String>, f64) {
        (
            labels
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            value,
        )
    }

    #[test]
    fn filesystems_count_each_device_once_fullest_first() {
        let fs = |mountpoint, device, value| {
            sample(
                &[
                    ("mountpoint", mountpoint),
                    ("device", device),
                    ("fstype", "ext4"),
                ],
                value,
            )
        };
        let listed = filesystems(
            vec![
                fs("/", "/dev/sda1", 100.0),
                fs("/etc/hostname", "/dev/sda1", 100.0),
                fs("/data", "/dev/sdb1", 1000.0),
            ],
            vec![
                fs("/", "/dev/sda1", 10.0),
                fs("/etc/hostname", "/dev/sda1", 10.0),
                fs("/data", "/dev/sdb1", 900.0),
            ],
        );
        let mountpoints: Vec<_> = listed.iter().map(|fs| fs.mountpoint.as_str()).collect();
        assert_eq!(mountpoints, ["/", "/data"]);
    }

    #[test]
    fn devices_are_busiest_first() {
        let device = |name, value| sample(&[("device", name)], value);
        let listed = devices(
            vec![device("eth0", 10.0), device("eth1", 500.0)],
            vec![device("eth0", 5.0), device("eth1", 1.0)],
        );
        assert_eq!(listed[0].device, "eth1");
        assert_eq!(listed[1].transmit_bytes_per_second, 5.0);
    }
}
