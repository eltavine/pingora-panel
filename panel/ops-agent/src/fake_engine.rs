//! Enough of the Engine API to answer what the agent asks, for its tests:
//! three containers, their logs and statistics, and the actions on them.

use crate::containers::{Engines, COMPOSE_PROJECT};
use axum::{
    extract::{Path as Segments, Query},
    http::StatusCode,
    response::{IntoResponse, Response as Answer},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// What the fake engine was asked to do, such as `stop b2`.
pub(crate) type Calls = Arc<Mutex<Vec<String>>>;

/// A container the fake engine knows, as inspecting it answers.
fn inspected(reference: &str) -> Option<Value> {
    let (id, name, running, project) = match reference {
        "b2" | "shop-web-1" => ("b2", "shop-web-1", true, Some("shop")),
        "a1" | "cache" => ("a1", "cache", false, None),
        "c3" | "pingora-panel-panel-api-1" => (
            "c3",
            "pingora-panel-panel-api-1",
            true,
            Some("pingora-panel"),
        ),
        _ => return None,
    };
    let mut labels = serde_json::Map::new();
    if let Some(project) = project {
        labels.insert(COMPOSE_PROJECT.into(), project.into());
    }
    Some(
        json!({"Id": id, "Name": format!("/{name}"), "RestartCount": 1,
                "Platform": "linux",
                "State": {"Status": if running { "running" } else { "exited" },
                          "Running": running, "ExitCode": 0, "OOMKilled": false,
                          "Error": "", "StartedAt": "2027-01-15T08:00:00Z",
                          "FinishedAt": "0001-01-01T00:00:00Z",
                          "Health": {"Status": "healthy"}},
                "Config": {"Labels": labels, "Hostname": "web", "User": "nginx",
                           "WorkingDir": "/srv",
                           "Env": ["DATABASE_PASSWORD=hunter2"],
                           "Cmd": ["nginx", "--token", "hunter2"]},
                "HostConfig": {"RestartPolicy": {"Name": "unless-stopped",
                                                 "MaximumRetryCount": 0}},
                "Mounts": [{"Type": "volume", "Name": "shop_html",
                            "Source": "/var/lib/docker/volumes/shop_html/_data",
                            "Destination": "/usr/share/nginx/html", "RW": false}],
                "NetworkSettings": {"Networks": {
                    "shop_default": {"IPAddress": "172.18.0.2", "Gateway": "172.18.0.1",
                                     "MacAddress": "02:42:ac:12:00:02",
                                     "Aliases": ["web"]}}}}),
    )
}

fn refusal(status: StatusCode, message: &str) -> Answer {
    (status, Json(json!({ "message": message }))).into_response()
}

/// What the engine reports a container uses: half of one of its four
/// CPUs, 200 MiB without the page cache, two interfaces' traffic and
/// some block I/O; read at the zero time when it is not running.
fn engine_stats(found: &Value) -> Value {
    let running = found["State"]["Running"] == true;
    let mib: u64 = 1024 * 1024;
    json!({
        "id": found["Id"], "name": found["Name"],
        "read": if running { "2027-01-15T08:00:01.5Z" } else { "0001-01-01T00:00:00Z" },
        "cpu_stats": {"cpu_usage": {"total_usage": 3_000_000_000_u64},
                      "system_cpu_usage": 104_000_000_000_u64, "online_cpus": 4},
        "precpu_stats": {"cpu_usage": {"total_usage": 2_500_000_000_u64},
                         "system_cpu_usage": 100_000_000_000_u64, "online_cpus": 4},
        "memory_stats": {"usage": 300 * mib, "limit": 8192 * mib,
                         "stats": {"inactive_file": 100 * mib}},
        "networks": {
            "eth0": {"rx_bytes": 1_000, "tx_bytes": 2_000, "rx_packets": 10,
                     "tx_packets": 20, "rx_errors": 1, "tx_errors": 0,
                     "rx_dropped": 0, "tx_dropped": 2},
            "eth1": {"rx_bytes": 500, "tx_bytes": 0, "rx_packets": 5, "tx_packets": 0}
        },
        "blkio_stats": {"io_service_bytes_recursive": [
            {"major": 8, "minor": 0, "op": "read", "value": 4_096},
            {"major": 8, "minor": 0, "op": "write", "value": 8_192}
        ]},
        "pids_stats": {"current": 5}
    })
}

/// What a container printed: its stream, when, and the text. The cache
/// has a terminal, so its output is not multiplexed.
fn printed(id: &str) -> Vec<(u8, u64, String)> {
    let at = |second: u64, text: &str| {
        let time = chrono::DateTime::from_timestamp(1_800_000_000 + second as i64, 1).unwrap();
        format!(
            "{} {text}",
            time.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        )
    };
    match id {
        "b2" => vec![
            (1, 0, at(0, "GET / 200")),
            (2, 1, at(1, "upstream timed out")),
            (1, 2, at(2, "GET /cart 200")),
        ],
        "a1" => vec![(0, 0, at(0, "Ready to accept connections"))],
        _ => (0..200)
            .map(|second| (1, second, at(second, &"x".repeat(16 * 1024))))
            .collect(),
    }
}

/// The Engine API's logs: the last `tail` entries, then those from
/// `since`, as frames or, for a terminal, as lines; following adds one.
fn logs(id: &str, query: &HashMap<String, String>) -> Vec<u8> {
    let number = |name: &str| query.get(name).and_then(|value| value.parse::<u64>().ok());
    let printed = printed(id);
    let tail = number("tail").map_or(printed.len(), |tail| tail as usize);
    let since = number("since").unwrap_or(0);
    let mut sent = printed[printed.len().saturating_sub(tail)..].to_vec();
    if query.get("follow").is_some_and(|follow| follow == "true") {
        let stream = if id == "a1" { 0 } else { 1 };
        sent.push((
            stream,
            3,
            "2027-01-15T08:00:03.000000001Z GET /new 200".into(),
        ));
    }
    let mut body = Vec::new();
    for (stream, second, text) in &sent {
        if since > 0 && 1_800_000_000 + second < since {
            continue;
        }
        let payload = format!("{text}\n");
        if *stream > 0 {
            body.extend([*stream, 0, 0, 0]);
            body.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
        }
        body.extend(payload.into_bytes());
    }
    body
}

/// Enough of the Engine API to answer what the agent asks.
pub(crate) async fn engine(directory: &Path) -> PathBuf {
    engine_with(directory, Calls::default()).await
}

pub(crate) async fn engine_with(directory: &Path, calls: Calls) -> PathBuf {
    let socket = directory.join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let router = Router::new()
        .route("/_ping", get(|| async { "OK" }))
        .route(
            "/version",
            get(|| async {
                Json(
                    json!({"Version": "28.3.3", "ApiVersion": "1.51", "Os": "linux",
                            "Arch": "amd64", "KernelVersion": "6.8.0",
                            "GoVersion": "go1.24.5"}),
                )
            }),
        )
        .route(
            "/info",
            get(|| async {
                Json(
                    json!({"Containers": 3, "ContainersRunning": 2, "ContainersPaused": 0,
                            "ContainersStopped": 1, "Images": 5, "Driver": "overlay2",
                            "CgroupDriver": "systemd", "OperatingSystem": "Ubuntu 24.04",
                            "NCPU": 4, "MemTotal": 8_589_934_592_u64, "Name": "web-1"}),
                )
            }),
        )
        .route(
            "/containers/json",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                let mut filters = query
                    .get("filters")
                    .and_then(|filters| {
                        serde_json::from_str::<HashMap<String, Vec<String>>>(filters).ok()
                    })
                    .unwrap_or_default();
                let ids = filters.remove("id").unwrap_or_default();
                let statuses = filters.remove("status").unwrap_or_default();
                let every = json!([
                    {"Id": "b2", "Names": ["/shop-web-1"], "Image": "nginx:1.27",
                     "ImageID": "sha256:aa", "Created": 1_800_000_000, "State": "running",
                     "Status": "Up 3 hours (healthy)",
                     "Ports": [{"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8081,
                                "Type": "tcp"}],
                     "Labels": {"com.docker.compose.project": "shop"}},
                    {"Id": "a1", "Names": ["/cache"], "Image": "redis:7",
                     "ImageID": "sha256:bb", "Created": 1_800_000_100, "State": "exited",
                     "Status": "Exited (0) 2 days ago", "Ports": [], "Labels": {}}
                ]);
                let listed: Vec<Value> = every
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|container| {
                        ids.is_empty() || ids.iter().any(|id| container["Id"] == id.as_str())
                    })
                    .filter(|container| {
                        statuses.is_empty()
                            || statuses
                                .iter()
                                .any(|state| container["State"] == state.as_str())
                    })
                    .cloned()
                    .collect();
                Json(listed)
            }),
        )
        .route(
            "/containers/{reference}/json",
            get(|Segments(reference): Segments<String>| async move {
                match inspected(&reference) {
                    Some(found) => Json(found).into_response(),
                    None => refusal(StatusCode::NOT_FOUND, "No such container"),
                }
            }),
        )
        .route(
            "/containers/{reference}/stats",
            get(|Segments(reference): Segments<String>| async move {
                match inspected(&reference) {
                    Some(found) => Json(engine_stats(&found)).into_response(),
                    None => refusal(StatusCode::NOT_FOUND, "No such container"),
                }
            }),
        )
        .route(
            "/containers/{reference}/logs",
            get(
                |Segments(reference): Segments<String>,
                 Query(query): Query<HashMap<String, String>>| async move {
                    match inspected(&reference) {
                        Some(found) => {
                            logs(found["Id"].as_str().unwrap_or_default(), &query).into_response()
                        }
                        None => refusal(StatusCode::NOT_FOUND, "No such container"),
                    }
                },
            ),
        );
    let acted = calls.clone();
    let router = router
        .route(
            "/containers/{reference}/{action}",
            post(
                move |Segments((reference, action)): Segments<(String, String)>| async move {
                    let Some(found) = inspected(&reference) else {
                        return refusal(StatusCode::NOT_FOUND, "No such container");
                    };
                    if action == "kill" && found["State"]["Running"] != true {
                        return refusal(StatusCode::CONFLICT, "container is not running");
                    }
                    acted.lock().unwrap().push(format!(
                        "{action} {}",
                        found["Id"].as_str().unwrap_or_default()
                    ));
                    StatusCode::NO_CONTENT.into_response()
                },
            ),
        )
        .route(
            "/containers/{reference}",
            delete(
                move |Segments(reference): Segments<String>,
                      Query(query): Query<HashMap<String, String>>| async move {
                    let Some(found) = inspected(&reference) else {
                        return refusal(StatusCode::NOT_FOUND, "No such container");
                    };
                    let force = query.get("force").is_some_and(|value| value == "true");
                    if found["State"]["Running"] == true && !force {
                        return refusal(
                            StatusCode::CONFLICT,
                            "You cannot remove a running container",
                        );
                    }
                    calls.lock().unwrap().push(format!(
                        "remove {} force={force} volumes={}",
                        found["Id"].as_str().unwrap_or_default(),
                        query.get("v").map_or("false", String::as_str)
                    ));
                    StatusCode::NO_CONTENT.into_response()
                },
            ),
        );
    let router = router.fallback(|uri: axum::http::Uri| async move {
        (axum::http::StatusCode::NOT_FOUND, format!("unrouted {uri}"))
    });
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    socket
}

pub(crate) fn engines(socket: PathBuf, state: Option<PathBuf>) -> Arc<Engines> {
    Arc::new(Engines::load(vec![("docker".into(), socket)], state))
}
