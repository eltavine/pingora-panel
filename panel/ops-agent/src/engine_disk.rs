//! The disk an engine's images, containers, volumes and build cache take,
//! as `docker system df` reports it (ADR 0031). The agent reads the
//! engine's own figures over its socket: Engine API 1.52 and later sum
//! them up for each kind, and earlier versions list every item, which the
//! agent sums as the Docker CLI does.

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use panel_contracts::ops::v1 as wire;
use panel_errors::PanelError;
use serde_json::Value;
use std::{path::Path, time::Duration};

/// How long the engine may take to add up its disk use; it walks every
/// volume to size it.
pub(crate) const USAGE_TIMEOUT: Duration = Duration::from_secs(60);

fn unreachable(error: impl std::fmt::Display) -> PanelError {
    PanelError::unavailable(format!("the engine: {error}"))
}

/// What `GET /system/df` answers, in the shape the engine's API gives it.
pub(crate) async fn data_usage(socket: &Path) -> Result<Value, PanelError> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(unreachable)?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(unreachable)?;
    let connection = tokio::spawn(connection);
    let request = http::Request::get("/system/df")
        .header(http::header::HOST, "engine")
        .body(Empty::<Bytes>::new())
        .map_err(|error| PanelError::internal(error.to_string()))?;
    let answered = async {
        let response = sender.send_request(request).await.map_err(unreachable)?;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(unreachable)?
            .to_bytes();
        Ok::<_, PanelError>((status, body))
    }
    .await;
    connection.abort();
    let (status, body) = answered?;
    if !status.is_success() {
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|answer| answer["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| status.to_string());
        return Err(unreachable(message));
    }
    serde_json::from_slice(&body)
        .map_err(|error| unreachable(format!("an unreadable answer: {error}")))
}

fn count(value: &Value) -> u32 {
    value
        .as_u64()
        .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
}

/// A size the engine reports, or none for one it reports as -1.
fn size(value: &Value) -> Option<u64> {
    value.as_i64().and_then(|size| u64::try_from(size).ok())
}

fn bytes(value: &Value) -> u64 {
    size(value).unwrap_or(0)
}

fn items(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// One kind's sums, as Engine API 1.52 and later report them.
fn summed(usage: &Value) -> wire::EngineDiskUse {
    wire::EngineDiskUse {
        total: count(&usage["TotalCount"]),
        active: count(&usage["ActiveCount"]),
        size_bytes: bytes(&usage["TotalSize"]),
        reclaimable_bytes: bytes(&usage["Reclaimable"]),
    }
}

/// Images from an earlier API's list: all their layers, less what images
/// containers use hold apart from shared layers.
fn images(df: &Value) -> wire::EngineDiskUse {
    let images = items(&df["Images"]);
    let layers = bytes(&df["LayersSize"]);
    let used = |image: &&Value| image["Containers"].as_i64().unwrap_or(0) > 0;
    let held: u64 = images
        .iter()
        .filter(used)
        .filter_map(|image| Some(size(&image["Size"])?.saturating_sub(size(&image["SharedSize"])?)))
        .sum();
    wire::EngineDiskUse {
        total: u32::try_from(images.len()).unwrap_or(u32::MAX),
        active: u32::try_from(images.iter().filter(used).count()).unwrap_or(u32::MAX),
        size_bytes: layers,
        reclaimable_bytes: layers.saturating_sub(held),
    }
}

/// Containers from an earlier API's list: what their writable layers hold.
fn containers(df: &Value) -> wire::EngineDiskUse {
    let containers = items(&df["Containers"]);
    let running = |container: &&Value| container["State"] == "running";
    let written = |container: &Value| bytes(&container["SizeRw"]);
    wire::EngineDiskUse {
        total: u32::try_from(containers.len()).unwrap_or(u32::MAX),
        active: u32::try_from(containers.iter().filter(running).count()).unwrap_or(u32::MAX),
        size_bytes: containers.iter().map(written).sum(),
        reclaimable_bytes: containers
            .iter()
            .filter(|container| !running(container))
            .map(written)
            .sum(),
    }
}

/// Volumes from an earlier API's list: what each holds and whether a
/// container mounts it.
fn volumes(df: &Value) -> wire::EngineDiskUse {
    let volumes = items(&df["Volumes"]);
    let mounted = |volume: &&Value| volume["UsageData"]["RefCount"].as_i64().unwrap_or(0) > 0;
    let held = |volume: &Value| bytes(&volume["UsageData"]["Size"]);
    wire::EngineDiskUse {
        total: u32::try_from(volumes.len()).unwrap_or(u32::MAX),
        active: u32::try_from(volumes.iter().filter(mounted).count()).unwrap_or(u32::MAX),
        size_bytes: volumes.iter().map(held).sum(),
        reclaimable_bytes: volumes
            .iter()
            .filter(|volume| !mounted(volume))
            .map(held)
            .sum(),
    }
}

/// The build cache from an earlier API's list: records neither in use nor
/// shared can go.
fn build_cache(df: &Value) -> wire::EngineDiskUse {
    let records = items(&df["BuildCache"]);
    let in_use = |record: &&Value| record["InUse"] == true;
    let held = |record: &Value| bytes(&record["Size"]);
    wire::EngineDiskUse {
        total: u32::try_from(records.len()).unwrap_or(u32::MAX),
        active: u32::try_from(records.iter().filter(in_use).count()).unwrap_or(u32::MAX),
        size_bytes: records.iter().map(held).sum(),
        reclaimable_bytes: records
            .iter()
            .filter(|record| !in_use(record) && record["Shared"] != true)
            .map(held)
            .sum(),
    }
}

/// Images, containers, volumes and the build cache, in that order.
pub(crate) fn usage(df: &Value) -> [wire::EngineDiskUse; 4] {
    if df.get("ImageUsage").is_some() {
        [
            summed(&df["ImageUsage"]),
            summed(&df["ContainerUsage"]),
            summed(&df["VolumeUsage"]),
            summed(&df["BuildCacheUsage"]),
        ]
    } else {
        [images(df), containers(df), volumes(df), build_cache(df)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_engines_sum_up_each_kind() {
        let df = json!({
            "ImageUsage": {"ActiveCount": 2, "TotalCount": 5, "Reclaimable": 300, "TotalSize": 1_000, "Items": []},
            "ContainerUsage": {"ActiveCount": 1, "TotalCount": 3, "Reclaimable": 20, "TotalSize": 50},
            "VolumeUsage": {"ActiveCount": 1, "TotalCount": 2, "Reclaimable": 0, "TotalSize": 4_096},
            "BuildCacheUsage": {"ActiveCount": 0, "TotalCount": 0, "Reclaimable": 0, "TotalSize": 0}
        });
        let [images, containers, volumes, cache] = usage(&df);
        assert_eq!(
            (
                images.total,
                images.active,
                images.size_bytes,
                images.reclaimable_bytes
            ),
            (5, 2, 1_000, 300)
        );
        assert_eq!((containers.total, containers.reclaimable_bytes), (3, 20));
        assert_eq!(volumes.size_bytes, 4_096);
        assert_eq!(cache, wire::EngineDiskUse::default());
    }

    #[test]
    fn earlier_engines_are_summed_as_the_docker_cli_does() {
        let df = json!({
            "LayersSize": 1_000,
            "Images": [
                {"Id": "sha256:aa", "Size": 600, "SharedSize": 100, "Containers": 1},
                {"Id": "sha256:bb", "Size": 300, "SharedSize": 100, "Containers": 0},
                {"Id": "sha256:cc", "Size": 200, "SharedSize": -1, "Containers": 2}
            ],
            "Containers": [
                {"Id": "b2", "State": "running", "SizeRw": 10},
                {"Id": "a1", "State": "exited", "SizeRw": 30},
                {"Id": "d4", "State": "created", "SizeRw": -1}
            ],
            "Volumes": [
                {"Name": "shop_html", "UsageData": {"Size": 4_096, "RefCount": 1}},
                {"Name": "orphan", "UsageData": {"Size": 1_024, "RefCount": 0}},
                {"Name": "remote", "UsageData": {"Size": -1, "RefCount": 0}}
            ],
            "BuildCache": [
                {"ID": "c1", "Size": 700, "InUse": false, "Shared": false},
                {"ID": "c2", "Size": 80, "InUse": true, "Shared": false},
                {"ID": "c3", "Size": 5, "InUse": false, "Shared": true}
            ]
        });
        let [images, containers, volumes, cache] = usage(&df);
        assert_eq!(
            (
                images.total,
                images.active,
                images.size_bytes,
                images.reclaimable_bytes
            ),
            (3, 2, 1_000, 500),
            "what images in use hold apart from shared layers stays"
        );
        assert_eq!(
            (
                containers.total,
                containers.active,
                containers.size_bytes,
                containers.reclaimable_bytes
            ),
            (3, 1, 40, 30)
        );
        assert_eq!(
            (
                volumes.total,
                volumes.active,
                volumes.size_bytes,
                volumes.reclaimable_bytes
            ),
            (3, 1, 5_120, 1_024)
        );
        assert_eq!(
            (
                cache.total,
                cache.active,
                cache.size_bytes,
                cache.reclaimable_bytes
            ),
            (3, 1, 785, 700)
        );
    }
}
