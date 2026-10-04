use super::*;
use panel_application::{
    CommandContext, ContainerAction, ContainerChange, ContainerDetail, ContainerEngine,
    ContainerFilter, ContainerList, ContainerMount, ContainerNetwork, ContainerState,
    ContainerSummary, ContainersPort, EngineInfo, EngineVersion, PortMapping, RequestScope,
};
use serde_json::Value;
use std::time::{Duration, UNIX_EPOCH};

/// A `docker` engine with two containers, and a `podman` one that does not
/// answer.
struct Engines;

fn docker(enabled: bool) -> ContainerEngine {
    ContainerEngine {
        id: "docker".into(),
        socket: "/run/docker.sock".into(),
        enabled,
        reachable: true,
        detail: String::new(),
        version: Some(EngineVersion {
            version: "28.3.3".into(),
            api_version: "1.51".into(),
            ..EngineVersion::default()
        }),
        info: Some(EngineInfo {
            containers: 2,
            running: 1,
            stopped: 1,
            ..EngineInfo::default()
        }),
    }
}

fn container(name: &str, image: &str, state: ContainerState) -> ContainerSummary {
    ContainerSummary {
        id: format!("{name}-id"),
        names: vec![name.into()],
        image: image.into(),
        image_id: "sha256:aa".into(),
        created: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        state,
        status: "Up 3 hours".into(),
        ports: vec![PortMapping {
            private_port: 80,
            public_port: Some(8081),
            host_ip: "0.0.0.0".into(),
            protocol: "tcp".into(),
        }],
        labels: [("com.docker.compose.project".to_owned(), "shop".to_owned())]
            .into_iter()
            .collect(),
        compose_project: Some("shop".into()),
    }
}

#[async_trait]
impl ContainersPort for Engines {
    async fn engines(&self, _scope: RequestScope) -> Result<Vec<ContainerEngine>> {
        Ok(vec![
            docker(true),
            ContainerEngine {
                id: "podman".into(),
                socket: "/run/podman/podman.sock".into(),
                enabled: true,
                reachable: false,
                detail: "the engine did not answer in time".into(),
                version: None,
                info: None,
            },
        ])
    }

    async fn set_engine(
        &self,
        _context: CommandContext,
        engine: String,
        enabled: bool,
    ) -> Result<ContainerEngine> {
        if engine != "docker" {
            return Err(PanelError::not_found(format!("no engine named {engine}")));
        }
        Ok(docker(enabled))
    }

    async fn containers(
        &self,
        _scope: RequestScope,
        _engine: String,
        filter: ContainerFilter,
    ) -> Result<ContainerList> {
        let all = vec![
            container("cache", "redis:7", ContainerState::Exited),
            container("shop-web-1", "nginx:1.27", ContainerState::Running),
        ];
        Ok(ContainerList {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            containers: all
                .into_iter()
                .filter(|item| filter.states.is_empty() || filter.states.contains(&item.state))
                .filter(|item| {
                    item.image.contains(&filter.search) || item.names[0].contains(&filter.search)
                })
                .collect(),
        })
    }

    async fn inspect(
        &self,
        _scope: RequestScope,
        _engine: String,
        reference: String,
    ) -> Result<ContainerDetail> {
        if reference != "shop-web-1" {
            return Err(PanelError::not_found(format!("no container {reference}")));
        }
        Ok(ContainerDetail {
            container: container(&reference, "nginx:1.27", ContainerState::Running),
            started_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            finished_at: None,
            exit_code: 0,
            error: None,
            oom_killed: false,
            restarts: 1,
            health: Some("healthy".into()),
            restart_policy: Some("unless-stopped".into()),
            restart_retries: 0,
            hostname: Some("web".into()),
            user: None,
            working_directory: Some("/srv".into()),
            platform: Some("linux".into()),
            mounts: vec![ContainerMount {
                kind: "volume".into(),
                name: Some("shop_html".into()),
                source: "/var/lib/docker/volumes/shop_html/_data".into(),
                destination: "/usr/share/nginx/html".into(),
                read_write: false,
            }],
            networks: vec![ContainerNetwork {
                name: "shop_default".into(),
                ip_address: Some("172.18.0.2".into()),
                ipv6_address: None,
                gateway: Some("172.18.0.1".into()),
                mac_address: None,
                aliases: vec!["web".into()],
            }],
        })
    }

    async fn act(
        &self,
        _context: CommandContext,
        _engine: String,
        reference: String,
        action: ContainerAction,
    ) -> Result<ContainerChange> {
        if reference == "pingora-panel-control-1" && action != ContainerAction::Start {
            return Err(PanelError::precondition_failed(
                "pingora-panel-control-1 belongs to the panel's installation; manage it with Compose",
            ));
        }
        if reference != "shop-web-1" {
            return Err(PanelError::not_found(format!("no container {reference}")));
        }
        let state = match action {
            ContainerAction::Start | ContainerAction::Restart => Some(ContainerState::Running),
            ContainerAction::Stop | ContainerAction::Kill => Some(ContainerState::Exited),
            ContainerAction::Remove { force: false, .. } => {
                return Err(PanelError::conflict(
                    "You cannot remove a running container",
                ));
            }
            ContainerAction::Remove { .. } => None,
        };
        Ok(ContainerChange {
            id: "shop-web-1-id".into(),
            name: reference.clone(),
            container: state.map(|state| container(&reference, "nginx:1.27", state)),
        })
    }
}

fn app(engines: bool) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(if engines {
        state.with_containers(Arc::new(Engines))
    } else {
        state
    })
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(app, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn post(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(
        app,
        Request::post(path)
            .header("x-actor", "ops")
            .header("idempotency-key", "engine-1")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn without_the_agent_engines_are_unsupported() {
    let (status, problem) = get(&app(false), "/api/v1/container-engines").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "UNSUPPORTED_CAPABILITY");
}

#[tokio::test]
async fn engines_are_listed_enabled_and_disabled() {
    let app = app(true);
    let (status, list) = get(&app, "/api/v1/container-engines").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["engines"][0]["id"], "docker");
    assert_eq!(list["engines"][0]["version"]["version"], "28.3.3");
    assert_eq!(list["engines"][0]["info"]["running"], 1);
    assert_eq!(list["engines"][0]["detail"], Value::Null);
    assert_eq!(list["engines"][1]["reachable"], false);
    assert_eq!(
        list["engines"][1]["detail"],
        "the engine did not answer in time"
    );

    let (status, engine) = post(&app, "/api/v1/container-engines/docker/disable").await;
    assert_eq!(status, StatusCode::OK, "{engine}");
    assert_eq!(engine["enabled"], false);
    let (status, problem) = post(&app, "/api/v1/container-engines/containerd/enable").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    let (status, problem) = post(&app, "/api/v1/container-engines/Docker1/enable").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}

#[tokio::test]
async fn containers_are_searched_and_filtered_by_state() {
    let app = app(true);
    let (status, list) = get(&app, "/api/v1/container-engines/docker/containers").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["observed_at"], "2027-01-15T08:00:00Z");
    assert_eq!(list["containers"].as_array().map(Vec::len), Some(2));

    let (status, list) = get(
        &app,
        "/api/v1/container-engines/docker/containers?state=running&search=nginx",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let shop = &list["containers"][0];
    assert_eq!(shop["names"][0], "shop-web-1");
    assert_eq!(shop["state"], "running");
    assert_eq!(shop["ports"][0]["public_port"], 8081);
    assert_eq!(shop["compose_project"], "shop");
    assert_eq!(shop["labels"]["com.docker.compose.project"], "shop");
    assert_eq!(list["containers"].as_array().map(Vec::len), Some(1));

    let (status, problem) = get(
        &app,
        "/api/v1/container-engines/docker/containers?state=sleeping",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}

async fn delete(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(
        app,
        Request::delete(path)
            .header("x-actor", "ops")
            .header("idempotency-key", "container-1")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn containers_are_started_stopped_restarted_killed_and_removed() {
    let app = app(true);
    for (action, state) in [
        ("stop", "exited"),
        ("start", "running"),
        ("restart", "running"),
        ("kill", "exited"),
    ] {
        let path = format!("/api/v1/container-engines/docker/containers/shop-web-1/{action}");
        let (status, change) = post(&app, &path).await;
        assert_eq!(status, StatusCode::OK, "{action}: {change}");
        assert_eq!(change["name"], "shop-web-1");
        assert_eq!(change["container"]["state"], state, "{action}");
    }

    let (status, problem) = delete(
        &app,
        "/api/v1/container-engines/docker/containers/shop-web-1",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    let (status, change) = delete(
        &app,
        "/api/v1/container-engines/docker/containers/shop-web-1?force=true&volumes=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{change}");
    assert_eq!(change["id"], "shop-web-1-id");
    assert_eq!(change["container"], Value::Null);
}

#[tokio::test]
async fn the_installation_and_unknown_actions_are_refused() {
    let engines = app(true);
    let installation = "/api/v1/container-engines/docker/containers/pingora-panel-control-1";
    let (status, problem) = post(&engines, &format!("{installation}/stop")).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{problem}");
    let (status, problem) = delete(&engines, installation).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{problem}");

    let (status, problem) = post(
        &engines,
        "/api/v1/container-engines/docker/containers/shop-web-1/pause",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let (status, problem) = post(
        &engines,
        "/api/v1/container-engines/docker/containers/-shop/stop",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let (status, problem) = post(
        &app(false),
        "/api/v1/container-engines/docker/containers/shop-web-1/start",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
}

#[tokio::test]
async fn a_container_is_inspected_with_its_labels_mounts_and_networks() {
    let engines = app(true);
    let (status, detail) = get(
        &engines,
        "/api/v1/container-engines/docker/containers/shop-web-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["container"]["names"][0], "shop-web-1");
    assert_eq!(
        detail["container"]["labels"]["com.docker.compose.project"],
        "shop"
    );
    assert_eq!(detail["started_at"], "2027-01-15T08:00:00Z");
    assert_eq!(detail["exit_code"], Value::Null);
    assert_eq!(detail["restart_policy"], "unless-stopped");
    assert_eq!(detail["user"], Value::Null);
    assert_eq!(detail["mounts"][0]["kind"], "volume");
    assert_eq!(detail["mounts"][0]["read_write"], false);
    assert_eq!(detail["networks"][0]["ip_address"], "172.18.0.2");

    let (status, problem) = get(
        &engines,
        "/api/v1/container-engines/docker/containers/nothing",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
}
