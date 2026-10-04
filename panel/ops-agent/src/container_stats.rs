//! What running containers use, read once from their engine as
//! `docker stats --no-stream` reads it (ADR 0031): the engine samples CPU
//! twice about a second apart and sends both samples.

use crate::containers::{failure, time};
use bollard::{
    models::{
        ContainerBlkioStats, ContainerCpuStats, ContainerMemoryStats,
        ContainerNetworkStats as EngineNetworkStats, ContainerStatsResponse,
    },
    query_parameters::{ListContainersOptionsBuilder, StatsOptionsBuilder},
    Docker,
};
use futures_util::{stream, StreamExt};
use panel_contracts::ops::v1 as wire;
use panel_errors::{ErrorCode, PanelError};
use std::{collections::HashMap, time::Duration};

/// How many containers are read at once; each takes the engine about a
/// second.
const CONCURRENT: usize = 16;
/// How long reading them may take.
pub(crate) const STATS_TIMEOUT: Duration = Duration::from_secs(25);

fn total(cpu: &ContainerCpuStats) -> u64 {
    cpu.cpu_usage
        .as_ref()
        .and_then(|usage| usage.total_usage)
        .unwrap_or(0)
}

/// How much of one CPU the container used between the two samples.
fn cpu_percent(now: Option<&ContainerCpuStats>, before: Option<&ContainerCpuStats>) -> f64 {
    let (Some(now), Some(before)) = (now, before) else {
        return 0.0;
    };
    let used = total(now).saturating_sub(total(before)) as f64;
    let elapsed = now
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(before.system_cpu_usage.unwrap_or(0)) as f64;
    let cpus = f64::from(online_cpus(now));
    if used > 0.0 && elapsed > 0.0 {
        used / elapsed * cpus * 100.0
    } else {
        0.0
    }
}

fn online_cpus(cpu: &ContainerCpuStats) -> u32 {
    cpu.online_cpus.unwrap_or_else(|| {
        cpu.cpu_usage
            .as_ref()
            .and_then(|usage| usage.percpu_usage.as_ref())
            .map_or(0, |cpus| u32::try_from(cpus.len()).unwrap_or(u32::MAX))
    })
}

/// Memory in use without the page cache the kernel can reclaim: cgroup
/// v1's `total_inactive_file`, or v2's `inactive_file`.
fn memory(stats: &ContainerMemoryStats) -> u64 {
    let usage = stats.usage.unwrap_or(0);
    let counters = stats.stats.as_ref();
    ["total_inactive_file", "inactive_file"]
        .iter()
        .find_map(|name| counters.and_then(|counters| counters.get(*name)))
        .filter(|inactive| **inactive < usage)
        .map_or(usage, |inactive| usage - inactive)
}

fn network(interfaces: &HashMap<String, EngineNetworkStats>) -> wire::ContainerNetworkStats {
    let sum = |field: fn(&EngineNetworkStats) -> Option<u64>| {
        interfaces
            .values()
            .map(|interface| field(interface).unwrap_or(0))
            .fold(0u64, u64::saturating_add)
    };
    wire::ContainerNetworkStats {
        received_bytes: sum(|interface| interface.rx_bytes),
        sent_bytes: sum(|interface| interface.tx_bytes),
        received_packets: sum(|interface| interface.rx_packets),
        sent_packets: sum(|interface| interface.tx_packets),
        errors: sum(|interface| interface.rx_errors)
            .saturating_add(sum(|interface| interface.tx_errors)),
        dropped: sum(|interface| interface.rx_dropped)
            .saturating_add(sum(|interface| interface.tx_dropped)),
    }
}

/// Bytes read and written, whatever case the engine names them in.
fn block_io(stats: &ContainerBlkioStats) -> (u64, u64) {
    let mut moved = (0u64, 0u64);
    for entry in stats.io_service_bytes_recursive.iter().flatten() {
        let value = entry.value.unwrap_or(0);
        match entry.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("read") => moved.0 = moved.0.saturating_add(value),
            Some("write") => moved.1 = moved.1.saturating_add(value),
            _ => {}
        }
    }
    moved
}

/// The statistics of a running container; none for one that is not
/// running, which the engine reports as read at the zero time.
fn stats(value: ContainerStatsResponse) -> Option<wire::ContainerStats> {
    let read_at = time(value.read.as_deref())?;
    let memory_stats = value.memory_stats.unwrap_or_default();
    let (block_read_bytes, block_written_bytes) = block_io(&value.blkio_stats.unwrap_or_default());
    Some(wire::ContainerStats {
        id: value.id.unwrap_or_default(),
        name: value
            .name
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_owned(),
        read_at: Some(read_at.into()),
        cpu_percent: cpu_percent(value.cpu_stats.as_ref(), value.precpu_stats.as_ref()),
        online_cpus: value.cpu_stats.as_ref().map_or(0, online_cpus),
        memory_bytes: memory(&memory_stats),
        memory_limit_bytes: memory_stats.limit.unwrap_or(0),
        network: value.networks.as_ref().map(network),
        block_read_bytes,
        block_written_bytes,
        pids: value.pids_stats.and_then(|pids| pids.current).unwrap_or(0),
    })
}

async fn read(client: &Docker, container: &str) -> Result<ContainerStatsResponse, PanelError> {
    let options = StatsOptionsBuilder::default()
        .stream(false)
        .one_shot(false)
        .build();
    let mut answers = std::pin::pin!(client.stats(container, Some(options)));
    answers
        .next()
        .await
        .ok_or_else(|| PanelError::unavailable("the engine sent no statistics"))?
        .map_err(|error| failure(&error))
}

/// One container's statistics; refused for one that is not running.
pub(crate) async fn one(
    client: &Docker,
    container: &str,
) -> Result<wire::ContainerStats, PanelError> {
    stats(read(client, container).await?)
        .ok_or_else(|| PanelError::precondition_failed(format!("{container} is not running")))
}

/// The statistics of every running container, by name. One that stops or
/// goes away while they are read is left out.
pub(crate) async fn running(client: &Docker) -> Result<Vec<wire::ContainerStats>, PanelError> {
    let filters = HashMap::from([("status", vec!["running"])]);
    let options = ListContainersOptionsBuilder::default()
        .filters(&filters)
        .build();
    let ids: Vec<String> = client
        .list_containers(Some(options))
        .await
        .map_err(|error| failure(&error))?
        .into_iter()
        .filter_map(|container| container.id)
        .collect();
    let read: Vec<Result<Option<wire::ContainerStats>, PanelError>> = stream::iter(ids)
        .map(|id| async move {
            match read(client, &id).await {
                Ok(value) => Ok(stats(value)),
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        ErrorCode::NOT_FOUND | ErrorCode::CONFLICT
                    ) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        })
        .buffer_unordered(CONCURRENT)
        .collect()
        .await;
    let mut running: Vec<wire::ContainerStats> = read
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    running.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerBlkioStatEntry, ContainerCpuUsage};

    fn cpu(total: u64, system: u64) -> ContainerCpuStats {
        ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(total),
                percpu_usage: Some(vec![0; 2]),
                ..ContainerCpuUsage::default()
            }),
            system_cpu_usage: Some(system),
            online_cpus: None,
            ..ContainerCpuStats::default()
        }
    }

    #[test]
    fn cpu_is_a_share_of_one_cpu_between_the_samples() {
        let before = cpu(1_000, 10_000);
        let now = cpu(2_000, 14_000);
        assert!((cpu_percent(Some(&now), Some(&before)) - 50.0).abs() < 1e-9);
        assert_eq!(online_cpus(&now), 2, "counted from the CPUs it reports");
        assert_eq!(cpu_percent(Some(&now), None), 0.0);
        assert_eq!(cpu_percent(Some(&before), Some(&now)), 0.0);
    }

    #[test]
    fn memory_leaves_out_the_page_cache() {
        let memory_of = |counter: &str, inactive: u64| {
            memory(&ContainerMemoryStats {
                usage: Some(1_000),
                stats: Some(HashMap::from([(counter.to_owned(), inactive)])),
                ..ContainerMemoryStats::default()
            })
        };
        assert_eq!(memory_of("inactive_file", 300), 700);
        assert_eq!(memory_of("total_inactive_file", 400), 600);
        assert_eq!(memory_of("inactive_file", 2_000), 1_000);
        assert_eq!(memory_of("active_file", 300), 1_000);
    }

    #[test]
    fn block_io_sums_reads_and_writes_in_any_case() {
        let entry = |op: &str, value: u64| ContainerBlkioStatEntry {
            op: Some(op.into()),
            value: Some(value),
            ..ContainerBlkioStatEntry::default()
        };
        let stats = ContainerBlkioStats {
            io_service_bytes_recursive: Some(vec![
                entry("read", 1),
                entry("Read", 2),
                entry("write", 4),
                entry("Total", 7),
            ]),
            ..ContainerBlkioStats::default()
        };
        assert_eq!(block_io(&stats), (3, 4));
    }
}
