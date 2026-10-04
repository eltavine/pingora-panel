//! The container engines the agent reaches, over their Engine API with
//! bollard (ADR 0031). Which sockets they are is the agent's configuration;
//! operators enable and disable them, and the agent keeps the choice.

use crate::{config::ENGINES_ENV, container_logs};
use bollard::{
    errors::Error as EngineError,
    models::{
        ContainerInspectResponse, ContainerSummary, ContainerSummaryStateEnum, HealthStatusEnum,
        PortSummary,
    },
    query_parameters::{
        InspectContainerOptions, KillContainerOptionsBuilder, ListContainersOptionsBuilder,
        RemoveContainerOptionsBuilder, RestartContainerOptions, StartContainerOptions,
        StopContainerOptions,
    },
    Docker, API_DEFAULT_VERSION,
};
use futures_util::{future::ready, stream::BoxStream, StreamExt};
use panel_contracts::ops::v1::{
    self as wire, containers_server::Containers, AgentCapability, Capability, CapabilityState,
    ContainerAction, ContainerState, Engine, EngineInfo, EngineVersion,
};
use panel_errors::PanelError;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};

/// How long an engine has to answer before it counts as unreachable.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an action may take; stopping waits out the container's stop
/// timeout before it kills.
pub(crate) const ACTION_TIMEOUT: Duration = Duration::from_secs(120);
/// The labels Compose puts on the containers it creates.
pub(crate) const COMPOSE_PROJECT: &str = "com.docker.compose.project";
pub(crate) const COMPOSE_SERVICE: &str = "com.docker.compose.service";
/// Where the agent keeps which engines are enabled, in its state directory.
const STATE_FILE: &str = "engines";
/// How many containers' logs may be followed at once; each holds a
/// connection to its engine.
const FOLLOWERS: usize = 64;

/// Whether the capability is enabled.
pub(crate) fn capability(engines: &[(String, PathBuf)]) -> AgentCapability {
    let (state, detail) = if engines.is_empty() {
        (
            CapabilityState::NotEnabled,
            format!("install containers.conf, which sets {ENGINES_ENV} and the socket's group"),
        )
    } else {
        (CapabilityState::Available, String::new())
    };
    AgentCapability {
        capability: Capability::Containers.into(),
        state: state.into(),
        detail,
    }
}

/// The configured engines and which of them operators enabled.
pub(crate) struct Engines {
    sockets: Vec<(String, PathBuf)>,
    state: Option<PathBuf>,
    enabled: Mutex<BTreeMap<String, bool>>,
}

impl Engines {
    /// Every configured engine starts enabled unless an operator disabled
    /// it before.
    pub(crate) fn load(sockets: Vec<(String, PathBuf)>, state: Option<PathBuf>) -> Self {
        let mut enabled: BTreeMap<String, bool> =
            sockets.iter().map(|(id, _)| (id.clone(), true)).collect();
        if let Some(text) = state
            .as_ref()
            .and_then(|directory| std::fs::read_to_string(directory.join(STATE_FILE)).ok())
        {
            for line in text.lines() {
                if let Some((id, value)) = line.split_once('=') {
                    if let Some(flag) = enabled.get_mut(id) {
                        *flag = value == "on";
                    }
                }
            }
        }
        Self {
            sockets,
            state,
            enabled: Mutex::new(enabled),
        }
    }

    fn socket(&self, id: &str) -> Result<&Path, PanelError> {
        self.sockets
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, socket)| socket.as_path())
            .ok_or_else(|| PanelError::not_found(format!("no engine named {id}")))
    }

    fn is_enabled(&self, id: &str) -> bool {
        self.enabled
            .lock()
            .map(|enabled| enabled.get(id).copied().unwrap_or(false))
            .unwrap_or(false)
    }

    fn client(socket: &Path) -> Result<Docker, PanelError> {
        Docker::connect_with_unix(
            &socket.display().to_string(),
            ENGINE_TIMEOUT.as_secs(),
            API_DEFAULT_VERSION,
        )
        .map_err(|error| failure(&error))
    }

    /// A client for an engine operators enabled.
    pub(crate) fn enabled(&self, id: &str) -> Result<Docker, PanelError> {
        let socket = self.socket(id)?;
        if !self.is_enabled(id) {
            return Err(PanelError::precondition_failed(format!(
                "the {id} engine is disabled"
            )));
        }
        Self::client(socket)
    }

    /// Enables or disables an engine and keeps the choice.
    pub(crate) fn set(&self, id: &str, enabled: bool) -> Result<(), PanelError> {
        self.socket(id)?;
        let mut flags = self
            .enabled
            .lock()
            .map_err(|_| PanelError::internal("the engine settings are unusable"))?;
        let mut changed = flags.clone();
        changed.insert(id.to_owned(), enabled);
        if let Some(directory) = &self.state {
            let text: String = changed
                .iter()
                .map(|(id, on)| format!("{id}={}\n", if *on { "on" } else { "off" }))
                .collect();
            let path = directory.join(STATE_FILE);
            let staged = directory.join(format!(".{STATE_FILE}.new"));
            std::fs::write(&staged, text)
                .and_then(|()| std::fs::rename(&staged, &path))
                .map_err(|error| {
                    PanelError::storage_unavailable(format!(
                        "cannot keep the engine settings in {}: {error}",
                        path.display()
                    ))
                })?;
        }
        *flags = changed;
        Ok(())
    }

    /// An engine with its version and figures, or why it does not answer.
    pub(crate) async fn describe(&self, id: &str) -> Result<Engine, PanelError> {
        let socket = self.socket(id)?;
        let mut engine = Engine {
            id: id.to_owned(),
            socket: socket.display().to_string(),
            enabled: self.is_enabled(id),
            ..Engine::default()
        };
        let read = async {
            let client = Self::client(socket)?;
            let version = client.version().await.map_err(|error| failure(&error))?;
            let info = client.info().await.map_err(|error| failure(&error))?;
            Ok::<_, PanelError>((version, info))
        };
        match tokio::time::timeout(ENGINE_TIMEOUT, read).await {
            Ok(Ok((version, info))) => {
                let count = |value: Option<i64>| u32::try_from(value.unwrap_or(0)).unwrap_or(0);
                engine.reachable = true;
                engine.version = Some(EngineVersion {
                    version: version.version.unwrap_or_default(),
                    api_version: version.api_version.unwrap_or_default(),
                    os: version.os.unwrap_or_default(),
                    architecture: version.arch.unwrap_or_default(),
                    kernel_version: version.kernel_version.unwrap_or_default(),
                    go_version: version.go_version.unwrap_or_default(),
                });
                engine.info = Some(EngineInfo {
                    containers: count(info.containers),
                    running: count(info.containers_running),
                    paused: count(info.containers_paused),
                    stopped: count(info.containers_stopped),
                    images: count(info.images),
                    storage_driver: info.driver.unwrap_or_default(),
                    cgroup_driver: info
                        .cgroup_driver
                        .map(|driver| driver.to_string())
                        .unwrap_or_default(),
                    operating_system: info.operating_system.unwrap_or_default(),
                    cpus: count(info.ncpu),
                    memory_bytes: u64::try_from(info.mem_total.unwrap_or(0)).unwrap_or(0),
                    name: info.name.unwrap_or_default(),
                });
            }
            Ok(Err(error)) => engine.detail = error.message,
            Err(_) => engine.detail = "the engine did not answer in time".into(),
        }
        Ok(engine)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &str> {
        self.sockets.iter().map(|(id, _)| id.as_str())
    }
}

/// An engine's error as the panel's, with the engine's own words.
pub(crate) fn failure(error: &EngineError) -> PanelError {
    match error {
        EngineError::DockerResponseServerError {
            status_code: 404,
            message,
        } => PanelError::not_found(message.clone()),
        EngineError::DockerResponseServerError {
            status_code: 409,
            message,
        } => PanelError::conflict(message.clone()),
        EngineError::DockerResponseServerError {
            status_code: 400,
            message,
        } => PanelError::invalid_argument(message.clone()),
        // Such as a log driver that keeps nothing to read back.
        EngineError::DockerResponseServerError {
            status_code: 501,
            message,
        } => PanelError::precondition_failed(message.clone()),
        error => PanelError::unavailable(format!("the engine: {error}")),
    }
}

fn state(value: Option<ContainerSummaryStateEnum>) -> ContainerState {
    match value {
        Some(ContainerSummaryStateEnum::CREATED) => ContainerState::Created,
        Some(ContainerSummaryStateEnum::RUNNING) => ContainerState::Running,
        Some(ContainerSummaryStateEnum::PAUSED) => ContainerState::Paused,
        Some(ContainerSummaryStateEnum::RESTARTING) => ContainerState::Restarting,
        Some(ContainerSummaryStateEnum::EXITED) => ContainerState::Exited,
        Some(ContainerSummaryStateEnum::REMOVING) => ContainerState::Removing,
        Some(ContainerSummaryStateEnum::DEAD) => ContainerState::Dead,
        Some(ContainerSummaryStateEnum::STOPPING) => ContainerState::Stopping,
        _ => ContainerState::Unspecified,
    }
}

fn port(value: PortSummary) -> wire::PortMapping {
    wire::PortMapping {
        private_port: value.private_port.into(),
        public_port: value.public_port.map(u32::from).unwrap_or(0),
        host_ip: value.ip.unwrap_or_default(),
        protocol: value
            .typ
            .map(|protocol| protocol.to_string())
            .unwrap_or_default(),
    }
}

pub(crate) fn container(value: ContainerSummary) -> wire::Container {
    let labels = value.labels.unwrap_or_default();
    wire::Container {
        id: value.id.unwrap_or_default(),
        names: value
            .names
            .unwrap_or_default()
            .into_iter()
            .map(|name| name.trim_start_matches('/').to_owned())
            .collect(),
        image: value.image.unwrap_or_default(),
        image_id: value.image_id.unwrap_or_default(),
        created: value
            .created
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(|seconds| (UNIX_EPOCH + Duration::from_secs(seconds)).into()),
        state: state(value.state).into(),
        status: value.status.unwrap_or_default(),
        ports: value
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(port)
            .collect(),
        compose_project: labels.get(COMPOSE_PROJECT).cloned().unwrap_or_default(),
        labels: labels.into_iter().collect(),
    }
}

/// An engine's timestamp; it reports the zero time for one that never was.
pub(crate) fn time(value: Option<&str>) -> Option<SystemTime> {
    value
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .filter(|moment| moment.timestamp() > 0)
        .map(SystemTime::from)
}

/// `healthy`, `unhealthy` or `starting`; empty without a health check.
pub(crate) fn health(status: Option<HealthStatusEnum>) -> String {
    match status {
        Some(HealthStatusEnum::STARTING) => "starting",
        Some(HealthStatusEnum::HEALTHY) => "healthy",
        Some(HealthStatusEnum::UNHEALTHY) => "unhealthy",
        _ => "",
    }
    .to_owned()
}

/// What inspecting a container shows, without its environment or command
/// line, which carry secrets.
fn detail(summary: wire::Container, inspected: ContainerInspectResponse) -> wire::ContainerDetail {
    let state = inspected.state.unwrap_or_default();
    let config = inspected.config.unwrap_or_default();
    let restart = inspected
        .host_config
        .and_then(|host| host.restart_policy)
        .unwrap_or_default();
    let mut networks: Vec<wire::ContainerNetwork> = inspected
        .network_settings
        .and_then(|settings| settings.networks)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, endpoint)| wire::ContainerNetwork {
            name,
            ip_address: endpoint.ip_address.unwrap_or_default(),
            ipv6_address: endpoint.global_ipv6_address.unwrap_or_default(),
            gateway: endpoint.gateway.unwrap_or_default(),
            mac_address: endpoint.mac_address.unwrap_or_default(),
            aliases: endpoint.aliases.unwrap_or_default(),
        })
        .collect();
    networks.sort_by(|left, right| left.name.cmp(&right.name));
    wire::ContainerDetail {
        container: Some(summary),
        started_at: time(state.started_at.as_deref()).map(Into::into),
        finished_at: time(state.finished_at.as_deref()).map(Into::into),
        exit_code: state.exit_code.unwrap_or(0),
        error: state.error.unwrap_or_default(),
        oom_killed: state.oom_killed.unwrap_or(false),
        restarts: u32::try_from(inspected.restart_count.unwrap_or(0)).unwrap_or(0),
        health: health(state.health.and_then(|health| health.status)),
        restart_policy: restart
            .name
            .map(|name| name.to_string())
            .unwrap_or_default(),
        restart_retries: u32::try_from(restart.maximum_retry_count.unwrap_or(0)).unwrap_or(0),
        hostname: config.hostname.unwrap_or_default(),
        user: config.user.unwrap_or_default(),
        working_directory: config.working_dir.unwrap_or_default(),
        platform: inspected.platform.unwrap_or_default(),
        mounts: inspected
            .mounts
            .unwrap_or_default()
            .into_iter()
            .map(|mount| wire::ContainerMount {
                r#type: mount.typ.unwrap_or_default(),
                name: mount.name.unwrap_or_default(),
                source: mount.source.unwrap_or_default(),
                destination: mount.destination.unwrap_or_default(),
                read_write: mount.rw.unwrap_or(false),
            })
            .collect(),
        networks,
    }
}

/// Whether a container answers a search and a set of states.
fn matches(container: &wire::Container, search: &str, states: &[i32]) -> bool {
    let search = search.trim().to_lowercase();
    (states.is_empty() || states.contains(&container.state))
        && (search.is_empty()
            || container.image.to_lowercase().contains(&search)
            || container
                .names
                .iter()
                .any(|name| name.to_lowercase().contains(&search)))
}

/// The engines and their containers to panel-api.
pub(crate) struct ContainerService {
    engines: Arc<Engines>,
    /// The Compose project of the panel's own installation.
    installation: String,
    followers: Arc<Semaphore>,
}

impl ContainerService {
    pub(crate) fn new(engines: Arc<Engines>, installation: String) -> Self {
        Self {
            engines,
            installation,
            followers: Arc::new(Semaphore::new(FOLLOWERS)),
        }
    }

    async fn read_logs(
        &self,
        request: &wire::ContainersLogsRequest,
    ) -> Result<(Vec<wire::ContainerLogLine>, bool), PanelError> {
        let reference = request.container.trim();
        if reference.is_empty() {
            return Err(PanelError::invalid_argument("name the container"));
        }
        let client = self.engines.enabled(&request.engine)?;
        let since = request
            .since
            .and_then(|since| SystemTime::try_from(since).ok());
        container_logs::read(&client, reference, request.lines, since).await
    }

    fn follow(
        &self,
        request: &wire::ContainersFollowLogsRequest,
    ) -> Result<BoxStream<'static, wire::ContainersFollowLogsResponse>, PanelError> {
        let reference = request.container.trim();
        if reference.is_empty() {
            return Err(PanelError::invalid_argument("name the container"));
        }
        let client = self.engines.enabled(&request.engine)?;
        let follower = Arc::clone(&self.followers)
            .try_acquire_owned()
            .map_err(|_| {
                PanelError::unavailable("too many logs are being followed; try again later")
            })?;
        let after = request
            .after
            .and_then(|after| SystemTime::try_from(after).ok());
        Ok(
            container_logs::follow(&client, reference, request.lines, after)
                .map(move |response| {
                    let _following = &follower;
                    response
                })
                .boxed(),
        )
    }

    /// A container as listing and inspecting it show.
    async fn inspect_one(
        &self,
        request: &wire::ContainersInspectRequest,
    ) -> Result<wire::ContainerDetail, PanelError> {
        let reference = request.container.trim();
        if reference.is_empty() {
            return Err(PanelError::invalid_argument("name the container"));
        }
        let client = self.engines.enabled(&request.engine)?;
        let inspected = client
            .inspect_container(reference, None::<InspectContainerOptions>)
            .await
            .map_err(|error| failure(&error))?;
        let id = inspected.id.clone().unwrap_or_default();
        let filters = HashMap::from([("id", vec![id.as_str()])]);
        let options = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        let summary = client
            .list_containers(Some(options))
            .await
            .map_err(|error| failure(&error))?
            .into_iter()
            .next()
            .map(container)
            .ok_or_else(|| PanelError::not_found(format!("{reference} is gone")))?;
        Ok(detail(summary, inspected))
    }

    /// Acts on a container, then reads it again unless it was removed.
    async fn act_on(
        &self,
        request: &wire::ContainersActRequest,
        action: ContainerAction,
    ) -> Result<wire::ContainersActResponse, PanelError> {
        let unspecified = || PanelError::invalid_argument("start, stop, restart, kill or remove");
        if action == ContainerAction::Unspecified {
            return Err(unspecified());
        }
        let reference = request.container.trim();
        if reference.is_empty() {
            return Err(PanelError::invalid_argument("name the container"));
        }
        let client = self
            .engines
            .enabled(&request.engine)?
            .with_timeout(ACTION_TIMEOUT);
        let found = client
            .inspect_container(reference, None::<InspectContainerOptions>)
            .await
            .map_err(|error| failure(&error))?;
        let id = found.id.unwrap_or_default();
        let name = found
            .name
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_owned();
        let project = found
            .config
            .and_then(|config| config.labels)
            .and_then(|mut labels| labels.remove(COMPOSE_PROJECT));
        if action != ContainerAction::Start && project.as_deref() == Some(&self.installation) {
            return Err(PanelError::precondition_failed(format!(
                "{name} belongs to the panel's installation; manage it with Compose"
            )));
        }
        let done = match action {
            ContainerAction::Start => {
                client
                    .start_container(&id, None::<StartContainerOptions>)
                    .await
            }
            ContainerAction::Stop => {
                client
                    .stop_container(&id, None::<StopContainerOptions>)
                    .await
            }
            ContainerAction::Restart => {
                client
                    .restart_container(&id, None::<RestartContainerOptions>)
                    .await
            }
            ContainerAction::Kill => {
                let options = KillContainerOptionsBuilder::default()
                    .signal("SIGKILL")
                    .build();
                client.kill_container(&id, Some(options)).await
            }
            ContainerAction::Remove => {
                let options = RemoveContainerOptionsBuilder::default()
                    .force(request.force)
                    .v(request.remove_volumes)
                    .build();
                client.remove_container(&id, Some(options)).await
            }
            ContainerAction::Unspecified => return Err(unspecified()),
        };
        done.map_err(|error| failure(&error))?;
        let container = if action == ContainerAction::Remove {
            None
        } else {
            let filters = HashMap::from([("id", vec![id.as_str()])]);
            let options = ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build();
            client
                .list_containers(Some(options))
                .await
                .map_err(|error| failure(&error))?
                .into_iter()
                .next()
                .map(container)
        };
        Ok(wire::ContainersActResponse {
            id,
            name,
            container,
            error: None,
        })
    }
}

fn answer<T>(
    result: Result<T, PanelError>,
) -> (Option<T>, Option<panel_contracts::common::v1::Error>) {
    match result {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some((&error).into())),
    }
}

#[tonic::async_trait]
impl Containers for ContainerService {
    type FollowLogsStream = BoxStream<'static, Result<wire::ContainersFollowLogsResponse, Status>>;
    async fn engines(
        &self,
        _: Request<wire::ContainersEnginesRequest>,
    ) -> Result<Response<wire::ContainersEnginesResponse>, Status> {
        let described =
            futures_util::future::join_all(self.engines.ids().map(|id| self.engines.describe(id)))
                .await;
        let (engines, error) = answer(described.into_iter().collect::<Result<Vec<_>, _>>());
        Ok(Response::new(wire::ContainersEnginesResponse {
            engines: engines.unwrap_or_default(),
            error,
        }))
    }

    async fn set_engine(
        &self,
        request: Request<wire::ContainersSetEngineRequest>,
    ) -> Result<Response<wire::ContainersSetEngineResponse>, Status> {
        let request = request.into_inner();
        let result = match self.engines.set(&request.engine, request.enabled) {
            Ok(()) => {
                tracing::info!(
                    event = "engine_set",
                    engine = %request.engine,
                    enabled = request.enabled,
                );
                self.engines.describe(&request.engine).await
            }
            Err(error) => Err(error),
        };
        let (engine, error) = answer(result);
        Ok(Response::new(wire::ContainersSetEngineResponse {
            engine,
            error,
        }))
    }

    async fn list(
        &self,
        request: Request<wire::ContainersListRequest>,
    ) -> Result<Response<wire::ContainersListResponse>, Status> {
        let request = request.into_inner();
        let listed = async {
            let client = self.engines.enabled(&request.engine)?;
            let options = ListContainersOptionsBuilder::default().all(true).build();
            let summaries = client
                .list_containers(Some(options))
                .await
                .map_err(|error| failure(&error))?;
            let mut containers: Vec<wire::Container> = summaries
                .into_iter()
                .map(container)
                .filter(|container| matches(container, &request.search, &request.states))
                .collect();
            containers.sort_by(|left, right| left.names.cmp(&right.names));
            Ok(containers)
        };
        let (containers, error) = answer(listed.await);
        Ok(Response::new(wire::ContainersListResponse {
            observed_at: containers.as_ref().map(|_| SystemTime::now().into()),
            containers: containers.unwrap_or_default(),
            error,
        }))
    }

    async fn inspect(
        &self,
        request: Request<wire::ContainersInspectRequest>,
    ) -> Result<Response<wire::ContainersInspectResponse>, Status> {
        let (detail, error) = answer(self.inspect_one(request.get_ref()).await);
        Ok(Response::new(wire::ContainersInspectResponse {
            detail,
            error,
        }))
    }

    async fn logs(
        &self,
        request: Request<wire::ContainersLogsRequest>,
    ) -> Result<Response<wire::ContainersLogsResponse>, Status> {
        let request = request.into_inner();
        let result = tokio::time::timeout(container_logs::READ_TIMEOUT, self.read_logs(&request))
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not send the logs in time",
                ))
            });
        let (read, error) = answer(result);
        let observed_at = read.as_ref().map(|_| SystemTime::now().into());
        let (lines, truncated) = read.unwrap_or_default();
        Ok(Response::new(wire::ContainersLogsResponse {
            observed_at,
            lines,
            truncated,
            error,
        }))
    }

    async fn follow_logs(
        &self,
        request: Request<wire::ContainersFollowLogsRequest>,
    ) -> Result<Response<Self::FollowLogsStream>, Status> {
        let responses = match self.follow(request.get_ref()) {
            Ok(responses) => responses,
            Err(error) => futures_util::stream::once(ready(wire::ContainersFollowLogsResponse {
                lines: Vec::new(),
                error: Some((&error).into()),
            }))
            .boxed(),
        };
        Ok(Response::new(responses.map(Ok).boxed()))
    }

    async fn act(
        &self,
        request: Request<wire::ContainersActRequest>,
    ) -> Result<Response<wire::ContainersActResponse>, Status> {
        let request = request.into_inner();
        let action =
            ContainerAction::try_from(request.action).unwrap_or(ContainerAction::Unspecified);
        let result = tokio::time::timeout(ACTION_TIMEOUT, self.act_on(&request, action))
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not finish in time",
                ))
            });
        Ok(Response::new(match result {
            Ok(done) => {
                tracing::info!(
                    event = "container_action",
                    engine = %request.engine,
                    container = %done.name,
                    action = action.as_str_name(),
                );
                done
            }
            Err(error) => {
                tracing::warn!(
                    event = "container_action_refused",
                    engine = %request.engine,
                    container = %request.container,
                    action = action.as_str_name(),
                    error_code = %error.code,
                );
                wire::ContainersActResponse {
                    error: Some((&error).into()),
                    ..wire::ContainersActResponse::default()
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path as Segments, Query},
        http::StatusCode,
        response::{IntoResponse, Response as Answer},
        routing::{delete, get, post},
        Json, Router,
    };
    use serde_json::{json, Value};

    /// What the fake engine was asked to do, such as `stop b2`.
    type Calls = Arc<Mutex<Vec<String>>>;

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
    async fn engine(directory: &Path) -> PathBuf {
        engine_with(directory, Calls::default()).await
    }

    async fn engine_with(directory: &Path, calls: Calls) -> PathBuf {
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
                    let ids = query
                        .get("filters")
                        .and_then(|filters| {
                            serde_json::from_str::<HashMap<String, Vec<String>>>(filters).ok()
                        })
                        .and_then(|mut filters| filters.remove("id"))
                        .unwrap_or_default();
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
                "/containers/{reference}/logs",
                get(
                    |Segments(reference): Segments<String>,
                     Query(query): Query<HashMap<String, String>>| async move {
                        match inspected(&reference) {
                            Some(found) => logs(found["Id"].as_str().unwrap_or_default(), &query)
                                .into_response(),
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

    fn engines(socket: PathBuf, state: Option<PathBuf>) -> Arc<Engines> {
        Arc::new(Engines::load(vec![("docker".into(), socket)], state))
    }

    async fn list(
        service: &ContainerService,
        search: &str,
        states: Vec<ContainerState>,
    ) -> wire::ContainersListResponse {
        service
            .list(Request::new(wire::ContainersListRequest {
                context: None,
                engine: "docker".into(),
                search: search.into(),
                states: states.into_iter().map(Into::into).collect(),
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn engines_report_their_version_and_figures() {
        let directory = tempfile::tempdir().unwrap();
        let socket = engine(directory.path()).await;
        let service = ContainerService::new(engines(socket.clone(), None), "pingora-panel".into());
        let engines = service
            .engines(Request::new(wire::ContainersEnginesRequest::default()))
            .await
            .unwrap()
            .into_inner()
            .engines;
        assert_eq!(engines.len(), 1);
        assert!(
            engines[0].reachable && engines[0].enabled,
            "{:?}",
            engines[0]
        );
        assert_eq!(engines[0].socket, socket.display().to_string());
        assert_eq!(engines[0].version.as_ref().unwrap().version, "28.3.3");
        let info = engines[0].info.as_ref().unwrap();
        assert_eq!((info.running, info.stopped, info.cpus), (2, 1, 4));
        assert_eq!(info.cgroup_driver, "systemd");

        let gone = Engines::load(
            vec![("podman".into(), directory.path().join("missing.sock"))],
            None,
        );
        let missing = gone.describe("podman").await.unwrap();
        assert!(!missing.reachable && !missing.detail.is_empty());
    }

    #[tokio::test]
    async fn containers_are_listed_searched_and_filtered() {
        let directory = tempfile::tempdir().unwrap();
        let service = ContainerService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );

        let every = list(&service, "", Vec::new()).await;
        assert!(
            every.error.is_none() && every.observed_at.is_some(),
            "{:?}",
            every.error
        );
        let names: Vec<_> = every
            .containers
            .iter()
            .map(|c| c.names[0].as_str())
            .collect();
        assert_eq!(names, ["cache", "shop-web-1"]);
        let shop = &every.containers[1];
        assert_eq!(shop.state(), ContainerState::Running);
        assert_eq!(shop.compose_project, "shop");
        assert_eq!(
            (shop.ports[0].private_port, shop.ports[0].public_port),
            (80, 8081)
        );
        assert_eq!(shop.ports[0].protocol, "tcp");
        assert_eq!(shop.created.unwrap().seconds, 1_800_000_000);

        let found = list(&service, "NGINX", Vec::new()).await;
        assert_eq!(found.containers.len(), 1);
        let exited = list(&service, "", vec![ContainerState::Exited]).await;
        assert_eq!(exited.containers[0].names, ["cache"]);
    }

    #[tokio::test]
    async fn disabled_engines_refuse_and_the_choice_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let socket = engine(directory.path()).await;
        let state = directory.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let service = ContainerService::new(
            engines(socket.clone(), Some(state.clone())),
            "pingora-panel".into(),
        );

        let set = service
            .set_engine(Request::new(wire::ContainersSetEngineRequest {
                context: None,
                engine: "docker".into(),
                enabled: false,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!set.engine.unwrap().enabled);
        let refused = list(&service, "", Vec::new()).await;
        assert_eq!(refused.error.unwrap().code, "PRECONDITION_FAILED");

        let reloaded = Engines::load(vec![("docker".into(), socket)], Some(state));
        assert!(
            reloaded.enabled("docker").is_err(),
            "the choice survives a restart"
        );

        let unknown = service
            .set_engine(Request::new(wire::ContainersSetEngineRequest {
                context: None,
                engine: "containerd".into(),
                enabled: true,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(unknown.error.unwrap().code, "NOT_FOUND");
    }

    async fn act(
        service: &ContainerService,
        container: &str,
        action: ContainerAction,
        force: bool,
    ) -> wire::ContainersActResponse {
        service
            .act(Request::new(wire::ContainersActRequest {
                context: None,
                engine: "docker".into(),
                container: container.into(),
                action: action.into(),
                force,
                remove_volumes: false,
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn containers_are_inspected_without_their_environment() {
        let directory = tempfile::tempdir().unwrap();
        let socket = engine(directory.path()).await;
        let service = ContainerService::new(engines(socket, None), "pingora-panel".into());
        let inspect = |container: &str| {
            service.inspect(Request::new(wire::ContainersInspectRequest {
                context: None,
                engine: "docker".into(),
                container: container.into(),
            }))
        };

        let answer = inspect("shop-web-1").await.unwrap().into_inner();
        let detail = answer.detail.unwrap();
        assert!(!format!("{detail:?}").contains("hunter2"), "{detail:?}");
        let summary = detail.container.as_ref().unwrap();
        assert_eq!(summary.names, vec!["shop-web-1".to_owned()]);
        assert_eq!(summary.ports[0].public_port, 8081);
        assert_eq!(
            (
                detail.restart_policy.as_str(),
                detail.restarts,
                detail.health.as_str()
            ),
            ("unless-stopped", 1, "healthy")
        );
        assert_eq!(
            (detail.hostname.as_str(), detail.user.as_str()),
            ("web", "nginx")
        );
        assert!(detail.started_at.is_some() && detail.finished_at.is_none());
        assert_eq!(detail.mounts[0].destination, "/usr/share/nginx/html");
        assert!(!detail.mounts[0].read_write);
        assert_eq!(detail.networks[0].name, "shop_default");
        assert_eq!(detail.networks[0].ip_address, "172.18.0.2");

        let missing = inspect("nothing").await.unwrap().into_inner();
        assert_eq!(missing.error.unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn containers_are_started_stopped_restarted_killed_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let socket = engine_with(directory.path(), calls.clone()).await;
        let service = ContainerService::new(engines(socket, None), "pingora-panel".into());

        let started = act(&service, "shop-web-1", ContainerAction::Start, false).await;
        assert!(started.error.is_none(), "{:?}", started.error);
        assert_eq!(
            (started.id.as_str(), started.name.as_str()),
            ("b2", "shop-web-1")
        );
        assert_eq!(started.container.unwrap().names, ["shop-web-1"]);
        for action in [
            ContainerAction::Stop,
            ContainerAction::Restart,
            ContainerAction::Kill,
        ] {
            let done = act(&service, "b2", action, false).await;
            assert!(done.error.is_none(), "{action:?}: {:?}", done.error);
        }
        let running = act(&service, "shop-web-1", ContainerAction::Remove, false).await;
        assert_eq!(running.error.unwrap().code, "CONFLICT");
        let removed = act(&service, "shop-web-1", ContainerAction::Remove, true).await;
        assert!(removed.error.is_none(), "{:?}", removed.error);
        assert!(removed.container.is_none());
        assert_eq!(removed.name, "shop-web-1");

        let exited = act(&service, "cache", ContainerAction::Kill, false).await;
        assert_eq!(exited.error.unwrap().code, "CONFLICT");
        let missing = act(&service, "ghost", ContainerAction::Start, false).await;
        assert_eq!(missing.error.unwrap().code, "NOT_FOUND");
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "start b2",
                "stop b2",
                "restart b2",
                "kill b2",
                "remove b2 force=true volumes=false"
            ]
        );
    }

    #[tokio::test]
    async fn the_installation_is_only_ever_started() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let socket = engine_with(directory.path(), calls.clone()).await;
        let service = ContainerService::new(engines(socket, None), "pingora-panel".into());
        for action in [
            ContainerAction::Stop,
            ContainerAction::Restart,
            ContainerAction::Kill,
            ContainerAction::Remove,
        ] {
            let refused = act(&service, "pingora-panel-panel-api-1", action, true).await;
            assert_eq!(
                refused.error.unwrap().code,
                "PRECONDITION_FAILED",
                "{action:?}"
            );
        }
        let started = act(
            &service,
            "pingora-panel-panel-api-1",
            ContainerAction::Start,
            false,
        )
        .await;
        assert!(started.error.is_none(), "{:?}", started.error);
        let unnamed = act(&service, "b2", ContainerAction::Unspecified, false).await;
        assert_eq!(unnamed.error.unwrap().code, "INVALID_ARGUMENT");
        assert_eq!(*calls.lock().unwrap(), ["start c3"]);
    }

    async fn read_logs(
        service: &ContainerService,
        container: &str,
        lines: u32,
        since: Option<SystemTime>,
    ) -> wire::ContainersLogsResponse {
        service
            .logs(Request::new(wire::ContainersLogsRequest {
                context: None,
                engine: "docker".into(),
                container: container.into(),
                lines,
                since: since.map(Into::into),
            }))
            .await
            .unwrap()
            .into_inner()
    }

    fn texts(lines: &[wire::ContainerLogLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    fn second(offset: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + offset)
    }

    #[tokio::test]
    async fn logs_are_read_with_their_streams_and_times() {
        let directory = tempfile::tempdir().unwrap();
        let service = ContainerService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );

        let every = read_logs(&service, "shop-web-1", 0, None).await;
        assert!(every.error.is_none(), "{:?}", every.error);
        assert!(every.observed_at.is_some() && !every.truncated);
        assert_eq!(
            texts(&every.lines),
            ["GET / 200", "upstream timed out", "GET /cart 200"]
        );
        assert_eq!(
            every.lines[1].stream(),
            wire::ContainerLogStream::Stderr,
            "standard error stays apart"
        );
        assert_eq!(
            every.lines[0].time.unwrap(),
            (second(0) + Duration::from_nanos(1)).into()
        );

        let last = read_logs(&service, "b2", 2, None).await;
        assert_eq!(texts(&last.lines), ["upstream timed out", "GET /cart 200"]);
        let since = read_logs(
            &service,
            "b2",
            0,
            Some(second(1) + Duration::from_millis(500)),
        )
        .await;
        assert_eq!(texts(&since.lines), ["GET /cart 200"]);

        let terminal = read_logs(&service, "cache", 0, None).await;
        assert_eq!(texts(&terminal.lines), ["Ready to accept connections"]);
        assert_eq!(terminal.lines[0].stream(), wire::ContainerLogStream::Stdout);

        let missing = read_logs(&service, "ghost", 0, None).await;
        assert_eq!(missing.error.unwrap().code, "NOT_FOUND");
        assert!(missing.observed_at.is_none());
    }

    #[tokio::test]
    async fn reading_logs_keeps_to_the_newest_two_mebibytes() {
        let directory = tempfile::tempdir().unwrap();
        let service = ContainerService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let noisy = read_logs(&service, "pingora-panel-panel-api-1", 0, None).await;
        assert!(noisy.truncated);
        let kept: usize = noisy.lines.iter().map(|line| line.text.len()).sum();
        assert!(kept <= 2 * 1024 * 1024, "{kept}");
        assert_eq!(
            noisy.lines.last().unwrap().time.unwrap(),
            (second(199) + Duration::from_nanos(1)).into(),
            "the newest lines are the ones kept"
        );
    }

    async fn follow(
        service: &ContainerService,
        container: &str,
        lines: u32,
        after: Option<SystemTime>,
    ) -> Vec<wire::ContainersFollowLogsResponse> {
        service
            .follow_logs(Request::new(wire::ContainersFollowLogsRequest {
                context: None,
                engine: "docker".into(),
                container: container.into(),
                lines,
                after: after.map(Into::into),
            }))
            .await
            .unwrap()
            .into_inner()
            .map(Result::unwrap)
            .collect()
            .await
    }

    #[tokio::test]
    async fn logs_are_followed_from_the_last_lines_or_after_a_line() {
        let directory = tempfile::tempdir().unwrap();
        let service = ContainerService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let lines = |responses: &[wire::ContainersFollowLogsResponse]| {
            assert!(responses.iter().all(|response| response.error.is_none()));
            responses
                .iter()
                .flat_map(|response| texts(&response.lines))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };

        let last = follow(&service, "shop-web-1", 1, None).await;
        assert_eq!(lines(&last), ["GET /cart 200", "GET /new 200"]);
        let none_before = follow(&service, "b2", 0, None).await;
        assert_eq!(lines(&none_before), ["GET /new 200"]);
        let after = second(1) + Duration::from_nanos(1);
        let resumed = follow(&service, "b2", 0, Some(after)).await;
        assert_eq!(
            lines(&resumed),
            ["GET /cart 200", "GET /new 200"],
            "a resumed follow starts after the line it left off at"
        );

        let missing = follow(&service, "ghost", 10, None).await;
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].error.as_ref().unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn followers_are_limited() {
        let directory = tempfile::tempdir().unwrap();
        let mut service = ContainerService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        service.followers = Arc::new(Semaphore::new(0));
        let refused = follow(&service, "b2", 0, None).await;
        assert_eq!(refused[0].error.as_ref().unwrap().code, "UNAVAILABLE");
    }

    #[test]
    fn a_choice_that_cannot_be_kept_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let engines = Engines::load(
            vec![("docker".into(), directory.path().join("docker.sock"))],
            Some(directory.path().join("missing")),
        );
        let refused = engines.set("docker", false).unwrap_err();
        assert_eq!(refused.code.as_str(), "STORAGE_UNAVAILABLE");
        assert!(engines.is_enabled("docker"));
    }

    #[test]
    fn the_capability_follows_the_configured_engines() {
        assert_eq!(capability(&[]).state(), CapabilityState::NotEnabled);
        assert_eq!(
            capability(&[("docker".into(), PathBuf::from("/run/docker.sock"))]).state(),
            CapabilityState::Available
        );
    }
}
