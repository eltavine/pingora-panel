//! The host the gateway runs on.

use crate::{
    client::{Api, Result},
    output::{bytes, percent, text, Column, Format, Output},
};
use serde_json::Value;
use std::time::Duration;

const SUMMARY: &[Column] = &[
    ("Host", |host| text(&host["hostname"])),
    ("System", |host| {
        format!(
            "{} · {} · {}",
            text(&host["operating_system"]),
            text(&host["kernel_release"]),
            text(&host["architecture"])
        )
    }),
    ("Clock", |host| {
        format!(
            "{} ({})",
            text(&host["host_time"]),
            text(&host["time_zone"])
        )
    }),
    ("Uptime", |host| match host["uptime_seconds"].as_u64() {
        Some(seconds) => humantime::format_duration(Duration::from_secs(seconds)).to_string(),
        None => "-".into(),
    }),
    ("CPU", |host| {
        format!(
            "{} of {} cores",
            percent(&host["cpu_usage"]),
            text(&host["cpu_count"])
        )
    }),
    ("Load", |host| {
        format!(
            "{} / {} / {}",
            text(&host["load1"]),
            text(&host["load5"]),
            text(&host["load15"])
        )
    }),
    ("Memory", |host| {
        let total = host["memory_total_bytes"].as_f64().unwrap_or_default();
        let used = total - host["memory_available_bytes"].as_f64().unwrap_or_default();
        format!(
            "{} of {} used",
            bytes(&Value::from(used)),
            bytes(&host["memory_total_bytes"])
        )
    }),
];

const FILESYSTEMS: &[Column] = &[
    ("MOUNT", |filesystem| text(&filesystem["mountpoint"])),
    ("DEVICE", |filesystem| text(&filesystem["device"])),
    ("SIZE", |filesystem| bytes(&filesystem["size_bytes"])),
    ("USED", |filesystem| percent(&filesystem["used_ratio"])),
    ("LEVEL", |filesystem| text(&filesystem["level"])),
];

const DEVICES: &[Column] = &[
    ("DEVICE", |device| text(&device["device"])),
    ("RECEIVE/S", |device| {
        bytes(&device["receive_bytes_per_second"])
    }),
    ("SEND/S", |device| {
        bytes(&device["transmit_bytes_per_second"])
    }),
];

pub async fn run(api: &Api, output: &Output) -> Result<()> {
    let host = api.get("/api/v1/host", &[]).await?.body;
    if output.format == Format::Json {
        output.json(&host);
        return Ok(());
    }
    if output.quiet {
        return Ok(());
    }
    if host["reporting"] != true {
        eprintln!("the host is not reporting figures; run the node exporter on it");
        return Ok(());
    }
    output.item(&host, SUMMARY);
    if host["filesystems"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        println!();
        output.list(&host["filesystems"], FILESYSTEMS);
    }
    if host["network_devices"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        println!();
        output.list(&host["network_devices"], DEVICES);
    }
    Ok(())
}
