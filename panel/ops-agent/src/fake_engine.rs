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

/// An image the fake engine has: nginx, which the running shop-web-1
/// uses; redis, which the stopped cache uses; and one nothing names.
fn image(reference: &str) -> Option<Value> {
    let (id, tags, digests, created) = match reference {
        "sha256:aa" | "nginx:1.27" => (
            "sha256:aa",
            json!(["nginx:1.27"]),
            json!(["nginx@sha256:d1"]),
            1_800_000_000,
        ),
        "sha256:bb" | "redis:7" => ("sha256:bb", json!(["redis:7"]), json!([]), 1_799_000_000),
        // What pulling gives it.
        "sha256:dd" | "busybox:1.37" => (
            "sha256:dd",
            json!(["busybox:1.37"]),
            json!(["busybox@sha256:d2"]),
            1_801_000_000,
        ),
        "sha256:ee" | "ghcr.io/example/private:2.3" => (
            "sha256:ee",
            json!(["ghcr.io/example/private:2.3"]),
            json!([]),
            1_801_000_000,
        ),
        "sha256:cc" => (
            "sha256:cc",
            json!(["<none>:<none>"]),
            json!(["<none>@<none>"]),
            1_798_000_000,
        ),
        _ => return None,
    };
    Some(json!({
        "Id": id, "ParentId": "", "RepoTags": tags, "RepoDigests": digests,
        "Created": created, "Size": 50_000_000, "SharedSize": -1, "Containers": -1,
        "Labels": {"maintainer": "NGINX"}
    }))
}

/// What inspecting an image answers, with an environment and command line
/// the agent must not pass on.
fn inspected_image(found: &Value) -> Value {
    json!({
        "Id": found["Id"], "RepoTags": found["RepoTags"], "RepoDigests": found["RepoDigests"],
        "Created": "2027-01-15T08:00:00Z", "Author": "NGINX Docker Maintainers",
        "Architecture": "amd64", "Os": "linux", "Size": found["Size"],
        "Config": {"User": "nginx", "WorkingDir": "/",
                   "ExposedPorts": {"80/tcp": {}, "443/tcp": {}},
                   "Volumes": {"/var/cache/nginx": {}},
                   "Env": ["NGINX_TOKEN=hunter2"], "Cmd": ["nginx", "--token", "hunter2"],
                   "StopSignal": "SIGQUIT", "Labels": {"maintainer": "NGINX"}},
        "RootFS": {"Type": "layers", "Layers": ["sha256:l1", "sha256:l2"]}
    })
}

/// Removing an image as the engine does: refused while a running container
/// uses it, and while a stopped one does unless forced.
fn removed_image(found: &Value, force: bool) -> Answer {
    match (found["Id"].as_str(), force) {
        (Some("sha256:aa"), _) => refusal(
            StatusCode::CONFLICT,
            "unable to delete sha256:aa (cannot be forced) - image is being used by running container b2",
        ),
        (Some("sha256:bb"), false) => refusal(
            StatusCode::CONFLICT,
            "unable to delete sha256:bb (must be forced) - image is being used by stopped container a1",
        ),
        (Some(id), _) => {
            let mut items: Vec<Value> = found["RepoTags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|tag| tag.as_str().is_some_and(|tag| !tag.starts_with("<none>")))
                .map(|tag| json!({"Untagged": tag}))
                .collect();
            items.push(json!({"Deleted": id}));
            Json(items).into_response()
        }
        (None, _) => refusal(StatusCode::NOT_FOUND, "No such image"),
    }
}

/// What pulling `name` at `tag` answers, as the engine streams it: busybox
/// downloads one layer and has another, nginx is up to date, the private
/// image wants `ci` signed in, missing does not exist and flaky breaks off.
fn pulled(name: &str, tag: &str, user: Option<&str>) -> Answer {
    let layers = |status: &str| {
        vec![
            json!({"status": format!("Pulling from {name}"), "id": tag}),
            json!({"status": "Already exists", "progressDetail": {}, "id": "1f2a3b4c5d6e"}),
            json!({"status": "Pulling fs layer", "progressDetail": {}, "id": "9c0abc9c5bd3"}),
            json!({"status": "Downloading", "progressDetail": {"current": 1024, "total": 4096},
                   "id": "9c0abc9c5bd3"}),
            json!({"status": "Download complete", "progressDetail": {}, "id": "9c0abc9c5bd3"}),
            json!({"status": "Extracting", "progressDetail": {"current": 4096, "total": 4096},
                   "id": "9c0abc9c5bd3"}),
            json!({"status": "Pull complete", "progressDetail": {}, "id": "9c0abc9c5bd3"}),
            json!({"status": "Digest: sha256:d2"}),
            json!({"status": format!("Status: {status} for {name}:{tag}")}),
        ]
    };
    let messages = match (name, tag) {
        ("busybox", "1.37") => layers("Downloaded newer image"),
        ("ghcr.io/example/private", "2.3") if user == Some("ci") => {
            layers("Downloaded newer image")
        }
        ("ghcr.io/example/private", _) => {
            return refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Head \"https://ghcr.io/v2/example/private/manifests/2.3\": unauthorized",
            )
        }
        ("nginx", "1.27") => vec![
            json!({"status": "Pulling from library/nginx", "id": "1.27"}),
            json!({"status": "Digest: sha256:d1"}),
            json!({"status": "Status: Image is up to date for nginx:1.27"}),
        ],
        ("flaky", _) => vec![
            json!({"status": "Pulling fs layer", "progressDetail": {}, "id": "9c0abc9c5bd3"}),
            json!({"errorDetail": {"message": "read: connection reset by peer"},
                   "error": "read: connection reset by peer"}),
        ],
        _ => {
            return refusal(
                StatusCode::NOT_FOUND,
                &format!("pull access denied for {name}, repository does not exist"),
            )
        }
    };
    let body: String = messages
        .iter()
        .map(|message| format!("{message}\r\n"))
        .collect();
    ([("content-type", "application/json")], body).into_response()
}

/// Whether labels answer a label filter: each `key` or `key=value` holds.
fn labelled(labels: &Value, filters: &[String]) -> bool {
    filters.iter().all(|filter| match filter.split_once('=') {
        Some((key, value)) => labels[key] == value,
        None => !labels[filter.as_str()].is_null(),
    })
}

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
                let labels = filters.remove("label").unwrap_or_default();
                let every = json!([
                    {"Id": "b2", "Names": ["/shop-web-1"], "Image": "nginx:1.27",
                     "ImageID": "sha256:aa", "Created": 1_800_000_000, "State": "running",
                     "SizeRw": 2_048,
                     "Status": "Up 3 hours (healthy)",
                     "Ports": [{"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8081,
                                "Type": "tcp"}],
                     "Labels": {"com.docker.compose.project": "shop",
                                "com.docker.compose.service": "web",
                                "com.docker.compose.project.working_dir": "/srv/shop",
                                "com.docker.compose.project.config_files":
                                    "/srv/shop/compose.yaml"},
                     "NetworkSettings": {"Networks": {
                         "shop_default": {"IPAddress": "172.18.0.2", "GlobalIPv6Address": ""}}},
                     "Mounts": [{"Type": "volume", "Name": "shop_html",
                                 "Destination": "/usr/share/nginx/html"},
                                {"Type": "bind", "Source": "/srv/shop", "Destination": "/srv"}]},
                    {"Id": "a1", "Names": ["/cache"], "Image": "redis:7", "SizeRw": 1_024,
                     "ImageID": "sha256:bb", "Created": 1_800_000_100, "State": "exited",
                     "Status": "Exited (0) 2 days ago", "Ports": [],
                     "Labels": {"com.docker.compose.project": "cache",
                                "com.docker.compose.service": "redis"}}
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
                    .filter(|container| labelled(&container["Labels"], &labels))
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
    let router = router
        .route(
            "/networks",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                let labels = query
                    .get("filters")
                    .and_then(|filters| {
                        serde_json::from_str::<HashMap<String, Vec<String>>>(filters).ok()
                    })
                    .and_then(|mut filters| filters.remove("label"))
                    .unwrap_or_default();
                let every = json!([
                    {"Name": "shop_default", "Id": "n3", "Created": "2027-01-15T08:00:00Z",
                     "Scope": "local", "Driver": "bridge", "Internal": false,
                     "Options": {"com.docker.network.enable_ipv6": "true"},
                     "IPAM": {"Config": [{"Subnet": "172.18.0.0/16", "Gateway": "172.18.0.1"}]},
                     "Labels": {"com.docker.compose.project": "shop"}},
                    {"Name": "bridge", "Id": "n1", "Scope": "local", "Driver": "bridge",
                     "IPAM": {"Config": [{"Subnet": "172.17.0.0/16"}]}, "Labels": {}},
                    {"Name": "host", "Id": "n2", "Scope": "local", "Driver": "host",
                     "IPAM": {"Config": []}, "Labels": {}},
                    {"Name": "stale_net", "Id": "n4", "Scope": "local", "Driver": "bridge",
                     "IPAM": {"Config": []}, "Labels": {}}
                ]);
                let listed: Vec<Value> = every
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|network| labelled(&network["Labels"], &labels))
                    .cloned()
                    .collect();
                Json(listed)
            }),
        )
        .route(
            "/system/df",
            get(|| async {
                Json(json!({
                    "LayersSize": 150_000_000,
                    "Images": [
                        {"Id": "sha256:aa", "Size": 50_000_000, "SharedSize": 0, "Containers": 1},
                        {"Id": "sha256:bb", "Size": 50_000_000, "SharedSize": 0, "Containers": 1},
                        {"Id": "sha256:cc", "Size": 50_000_000, "SharedSize": 0, "Containers": 0}
                    ],
                    "Containers": [
                        {"Id": "b2", "State": "running", "SizeRw": 2_048},
                        {"Id": "a1", "State": "exited", "SizeRw": 1_024}
                    ],
                    "Volumes": [
                        {"Name": "shop_html", "UsageData": {"Size": 4_096, "RefCount": 1}},
                        {"Name": "orphan", "UsageData": {"Size": 1_024, "RefCount": 0}},
                        {"Name": "3f9a1c", "UsageData": {"Size": 2_048, "RefCount": 0}}
                    ],
                    "BuildCache": [
                        {"ID": "c1", "Type": "regular", "InUse": false, "Shared": false,
                         "Size": 700, "Description": "mount / from exec /bin/sh -c make"},
                        {"ID": "c2", "Type": "regular", "InUse": true, "Shared": false, "Size": 80}
                    ]
                }))
            }),
        )
        .route(
            "/volumes",
            get(|| async {
                Json(json!({"Volumes": [
                    {"Name": "shop_html", "Driver": "local", "Scope": "local",
                     "Mountpoint": "/var/lib/docker/volumes/shop_html/_data",
                     "CreatedAt": "2027-01-15T08:00:00Z", "Options": {},
                     "Labels": {"com.docker.compose.project": "shop"}},
                    {"Name": "orphan", "Driver": "local", "Scope": "local",
                     "Mountpoint": "/var/lib/docker/volumes/orphan/_data",
                     "Options": {}, "Labels": {}},
                    {"Name": "3f9a1c", "Driver": "local", "Scope": "local",
                     "Mountpoint": "/var/lib/docker/volumes/3f9a1c/_data",
                     "Options": {}, "Labels": {"com.docker.volume.anonymous": ""}}
                ], "Warnings": []}))
            }),
        );
    let (volumes_gone, networks_gone, cache_gone) = (calls.clone(), calls.clone(), calls.clone());
    let router = router
        .route(
            "/volumes/{name}",
            delete(move |Segments(name): Segments<String>| async move {
                volumes_gone
                    .lock()
                    .unwrap()
                    .push(format!("remove-volume {name}"));
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/networks/{id}",
            delete(move |Segments(id): Segments<String>| async move {
                networks_gone
                    .lock()
                    .unwrap()
                    .push(format!("remove-network {id}"));
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/build/prune",
            post(
                move |Query(query): Query<HashMap<String, String>>| async move {
                    let ids = query
                        .get("filters")
                        .and_then(|filters| {
                            serde_json::from_str::<HashMap<String, Vec<String>>>(filters).ok()
                        })
                        .and_then(|mut filters| filters.remove("id"))
                        .unwrap_or_default();
                    let deleted: Vec<&String> = ids.iter().filter(|id| *id == "c1").collect();
                    cache_gone
                        .lock()
                        .unwrap()
                        .extend(deleted.iter().map(|id| format!("prune-build {id}")));
                    Json(json!({"CachesDeleted": deleted, "SpaceReclaimed": 700}))
                },
            ),
        );
    let (removing, pulling) = (calls.clone(), calls.clone());
    let router = router.route(
        "/images/{*rest}",
        get(|Segments(rest): Segments<String>| async move {
            if rest == "json" {
                let every: Vec<Value> = ["sha256:aa", "sha256:bb", "sha256:cc"]
                    .into_iter()
                    .filter_map(image)
                    .collect();
                return Json(every).into_response();
            }
            match rest.strip_suffix("/json").and_then(image) {
                Some(found) => Json(inspected_image(&found)).into_response(),
                None => refusal(StatusCode::NOT_FOUND, "No such image"),
            }
        })
        .delete(
            move |Segments(name): Segments<String>,
                  Query(query): Query<HashMap<String, String>>| async move {
                let Some(found) = image(&name) else {
                    return refusal(StatusCode::NOT_FOUND, "No such image");
                };
                let force = query.get("force").is_some_and(|value| value == "true");
                removing
                    .lock()
                    .unwrap()
                    .push(format!("remove-image {name} force={force}"));
                removed_image(&found, force)
            },
        )
        .post(
            move |Segments(rest): Segments<String>,
                  Query(query): Query<HashMap<String, String>>,
                  headers: axum::http::HeaderMap| async move {
                if rest != "create" {
                    return refusal(StatusCode::NOT_FOUND, "unrouted");
                }
                use base64::Engine as _;
                let user = headers
                    .get("x-registry-auth")
                    .and_then(|value| {
                        base64::engine::general_purpose::STANDARD
                            .decode(value.as_bytes())
                            .ok()
                    })
                    .and_then(|json| serde_json::from_slice::<Value>(&json).ok())
                    .filter(|auth| auth["password"] == "hunter2")
                    .and_then(|auth| auth["username"].as_str().map(str::to_owned));
                let field = |name: &str| query.get(name).cloned().unwrap_or_default();
                let (name, tag) = (field("fromImage"), field("tag"));
                pulling.lock().unwrap().push(format!(
                    "pull {name} {tag} platform={} user={}",
                    field("platform"),
                    user.as_deref().unwrap_or("-")
                ));
                pulled(&name, &tag, user.as_deref())
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
