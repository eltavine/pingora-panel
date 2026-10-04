#![forbid(unsafe_code)]

//! Host figures read from a stand-in Prometheus that answers as the node
//! exporter's metrics would.

use axum::{extract::Query, routing::get, Json, Router};
use observability_service::{prometheus, HostService};
use serde_json::{json, Value};
use std::collections::HashMap;

fn sample(labels: Value, value: f64) -> Value {
    json!({ "metric": labels, "value": [1_700_000_000, value.to_string()] })
}

/// What the node exporter's metrics would answer, by query.
fn answer(query: &str, reporting: bool) -> Vec<Value> {
    if !reporting {
        return Vec::new();
    }
    let one = |value| vec![sample(json!({}), value)];
    let filesystem = |mountpoint: &str, device: &str, value| {
        sample(
            json!({"mountpoint": mountpoint, "device": device, "fstype": "ext4"}),
            value,
        )
    };
    let device = |name: &str, value| sample(json!({ "device": name }), value);
    if query.starts_with("node_uname_info") {
        vec![sample(
            json!({"nodename": "web-1", "release": "6.8.0-41-generic", "machine": "x86_64"}),
            1.0,
        )]
    } else if query.starts_with("node_os_info") {
        vec![sample(json!({"pretty_name": "Ubuntu 24.04.2 LTS"}), 1.0)]
    } else if query.starts_with("node_time_zone_offset_seconds") {
        vec![sample(json!({"time_zone": "UTC"}), 0.0)]
    } else if query.starts_with("node_time_seconds") {
        one(1_700_000_000.0)
    } else if query.starts_with("node_boot_time_seconds") {
        one(1_699_913_600.0)
    } else if query.starts_with("count(node_cpu_seconds_total") {
        one(4.0)
    } else if query.starts_with("1 - avg(rate(node_cpu_seconds_total") {
        one(0.25)
    } else if query.starts_with("node_load1{") {
        one(0.5)
    } else if query.starts_with("node_load5{") {
        one(0.75)
    } else if query.starts_with("node_load15{") {
        one(1.0)
    } else if query.starts_with("node_memory_MemTotal_bytes") {
        one(8e9)
    } else if query.starts_with("node_memory_MemAvailable_bytes") {
        one(2e9)
    } else if query.starts_with("node_filesystem_size_bytes") {
        vec![
            filesystem("/", "/dev/sda1", 100e9),
            filesystem("/data", "/dev/sdb1", 500e9),
        ]
    } else if query.starts_with("node_filesystem_avail_bytes") {
        vec![
            filesystem("/", "/dev/sda1", 8e9),
            filesystem("/data", "/dev/sdb1", 400e9),
        ]
    } else if query.contains("node_network_receive_bytes_total") {
        vec![device("eth0", 1200.0)]
    } else if query.contains("node_network_transmit_bytes_total") {
        vec![device("eth0", 300.0)]
    } else {
        Vec::new()
    }
}

async fn stand_in(reporting: bool) -> String {
    let query = move |Query(params): Query<HashMap<String, String>>| async move {
        let result = answer(params.get("query").map_or("", String::as_str), reporting);
        Json(json!({"status": "success", "data": {"resultType": "vector", "result": result}}))
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/api/v1/query", get(query).post(query));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn host_figures_come_from_the_node_exporter() {
    let host = HostService::new(prometheus(&stand_in(true).await).unwrap());
    let summary = host.summarize().await.unwrap();
    assert!(summary.reporting);
    assert_eq!(summary.hostname, "web-1");
    assert_eq!(summary.operating_system, "Ubuntu 24.04.2 LTS");
    assert_eq!(
        (
            summary.kernel_release.as_str(),
            summary.architecture.as_str()
        ),
        ("6.8.0-41-generic", "x86_64")
    );
    assert_eq!(summary.time_zone, "UTC");
    assert_eq!(summary.uptime.unwrap().seconds, 86_400);
    assert_eq!((summary.cpu_count, summary.cpu_usage), (4, Some(0.25)));
    assert_eq!(
        (summary.load1, summary.load5, summary.load15),
        (0.5, 0.75, 1.0)
    );
    assert_eq!(
        (summary.memory_total_bytes, summary.memory_available_bytes),
        (8e9, 2e9)
    );
    let mountpoints: Vec<_> = summary
        .filesystems
        .iter()
        .map(|filesystem| filesystem.mountpoint.as_str())
        .collect();
    assert_eq!(mountpoints, ["/", "/data"], "fullest first");
    assert_eq!(summary.network_devices[0].receive_bytes_per_second, 1200.0);
    assert_eq!(summary.network_devices[0].transmit_bytes_per_second, 300.0);
}

#[tokio::test]
async fn hosts_without_an_exporter_report_nothing() {
    let host = HostService::new(prometheus(&stand_in(false).await).unwrap());
    let summary = host.summarize().await.unwrap();
    assert!(!summary.reporting);
    assert!(summary.hostname.is_empty());
    assert!(summary.observed_at.is_some());
}
