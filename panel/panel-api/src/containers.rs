//! The container engines the host agent reaches and what runs on them
//! (ADR 0031).

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    tail::{relay, LogTailError, Relayed},
    time::parse_time,
    ApiState,
};
use axum::{
    extract::{
        ws::{CloseFrame, Utf8Bytes, WebSocketUpgrade},
        Path, Query, State,
    },
    http::HeaderMap,
    response::Response,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{future::ready, Stream, StreamExt};
use panel_application::{
    ContainerAction, ContainerChange, ContainerDetail, ContainerEngine, ContainerFilter,
    ContainerList, ContainerLogLine, ContainerLogQuery, ContainerLogStart, ContainerLogStream,
    ContainerLogTail, ContainerLogs, ContainerMount, ContainerNetwork, ContainerNetworkStats,
    ContainerState, ContainerStats, ContainerStatsList, ContainerSummary, EngineInfo,
    EngineVersion, PortMapping,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A line's time, as precisely as its engine recorded it.
fn precise(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The most lines a read returns.
const MOST_LINES: u32 = 5_000;
/// The lines a read returns unless asked for another number.
const DEFAULT_LINES: u32 = 200;
/// The most lines sent before following.
const MOST_BACKLOG: u32 = 1_000;
/// The lines sent before following unless asked for another number.
const DEFAULT_BACKLOG: u32 = 100;

/// An engine's name as a path names it: lowercase letters only.
fn engine(name: String) -> Result<String, ApiError> {
    if !name.is_empty() && name.len() <= 32 && name.bytes().all(|byte| byte.is_ascii_lowercase()) {
        Ok(name)
    } else {
        Err(ApiError::new(PanelError::invalid_argument(format!(
            "`{name}` is not an engine's name"
        ))))
    }
}

/// A container as a path names it: its ID, a unique prefix of its ID or its
/// name, by the characters the engines allow in either.
fn container(reference: String) -> Result<String, ApiError> {
    let valid = !reference.is_empty()
        && reference.len() <= 128
        && reference.starts_with(|c: char| c.is_ascii_alphanumeric())
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(reference)
    } else {
        Err(ApiError::new(PanelError::invalid_argument(format!(
            "`{reference}` is not a container's ID or name"
        ))))
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineVersionView {
    /// Such as `28.3.3`.
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub architecture: String,
    pub kernel_version: String,
    pub go_version: String,
}

impl From<EngineVersion> for EngineVersionView {
    fn from(value: EngineVersion) -> Self {
        Self {
            version: value.version,
            api_version: value.api_version,
            os: value.os,
            architecture: value.architecture,
            kernel_version: value.kernel_version,
            go_version: value.go_version,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineInfoView {
    pub containers: u32,
    pub running: u32,
    pub paused: u32,
    pub stopped: u32,
    pub images: u32,
    pub storage_driver: String,
    pub cgroup_driver: String,
    pub operating_system: String,
    pub cpus: u32,
    pub memory_bytes: u64,
    /// The host's name as the engine reports it.
    pub name: String,
}

impl From<EngineInfo> for EngineInfoView {
    fn from(value: EngineInfo) -> Self {
        Self {
            containers: value.containers,
            running: value.running,
            paused: value.paused,
            stopped: value.stopped,
            images: value.images,
            storage_driver: value.storage_driver,
            cgroup_driver: value.cgroup_driver,
            operating_system: value.operating_system,
            cpus: value.cpus,
            memory_bytes: value.memory_bytes,
            name: value.name,
        }
    }
}

/// A Docker or Podman engine the host agent is configured with.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerEngineView {
    /// `docker` or `podman`.
    pub id: String,
    /// The engine's socket on the host.
    pub socket: String,
    pub enabled: bool,
    pub reachable: bool,
    /// Why it is unreachable.
    pub detail: Option<String>,
    pub version: Option<EngineVersionView>,
    pub info: Option<EngineInfoView>,
}

impl From<ContainerEngine> for ContainerEngineView {
    fn from(value: ContainerEngine) -> Self {
        Self {
            id: value.id,
            socket: value.socket,
            enabled: value.enabled,
            reachable: value.reachable,
            detail: (!value.detail.is_empty()).then_some(value.detail),
            version: value.version.map(Into::into),
            info: value.info.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerEngineList {
    pub engines: Vec<ContainerEngineView>,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContainerStateName {
    Created,
    Running,
    Paused,
    Restarting,
    Exited,
    Removing,
    Dead,
    Stopping,
    /// A state the panel does not know yet.
    Unknown,
}

impl From<ContainerState> for ContainerStateName {
    fn from(value: ContainerState) -> Self {
        match value {
            ContainerState::Created => Self::Created,
            ContainerState::Running => Self::Running,
            ContainerState::Paused => Self::Paused,
            ContainerState::Restarting => Self::Restarting,
            ContainerState::Exited => Self::Exited,
            ContainerState::Removing => Self::Removing,
            ContainerState::Dead => Self::Dead,
            ContainerState::Stopping => Self::Stopping,
            ContainerState::Unknown => Self::Unknown,
        }
    }
}

fn state(name: &str) -> Result<ContainerState, ApiError> {
    Ok(match name.trim() {
        "created" => ContainerState::Created,
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "restarting" => ContainerState::Restarting,
        "exited" => ContainerState::Exited,
        "removing" => ContainerState::Removing,
        "dead" => ContainerState::Dead,
        "stopping" => ContainerState::Stopping,
        other => {
            return Err(ApiError::new(PanelError::invalid_argument(format!(
                "`{other}` is not a container state"
            ))))
        }
    })
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PortMappingView {
    /// The container's port.
    pub private_port: u16,
    /// The host's port, when published.
    pub public_port: Option<u16>,
    /// The host address it is published on, such as `0.0.0.0`.
    pub host_ip: String,
    /// `tcp`, `udp` or `sctp`.
    pub protocol: String,
}

impl From<PortMapping> for PortMappingView {
    fn from(value: PortMapping) -> Self {
        Self {
            private_port: value.private_port,
            public_port: value.public_port,
            host_ip: value.host_ip,
            protocol: value.protocol,
        }
    }
}

/// A container as a list shows it, without its command line.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerView {
    pub id: String,
    /// Without the leading slash.
    pub names: Vec<String>,
    pub image: String,
    pub image_id: String,
    /// RFC 3339.
    pub created: Option<String>,
    pub state: ContainerStateName,
    /// Such as `Up 3 hours (healthy)`.
    pub status: String,
    pub ports: Vec<PortMappingView>,
    pub labels: BTreeMap<String, String>,
    /// The Compose project that created it, if one did.
    pub compose_project: Option<String>,
}

impl From<ContainerSummary> for ContainerView {
    fn from(value: ContainerSummary) -> Self {
        Self {
            id: value.id,
            names: value.names,
            image: value.image,
            image_id: value.image_id,
            created: value.created.map(rfc3339),
            state: value.state.into(),
            status: value.status,
            ports: value.ports.into_iter().map(Into::into).collect(),
            labels: value.labels,
            compose_project: value.compose_project,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// By name.
    pub containers: Vec<ContainerView>,
}

impl From<ContainerList> for ContainerListView {
    fn from(value: ContainerList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            containers: value.containers.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContainersQuery {
    /// Matched against names and images, ignoring case.
    search: Option<String>,
    /// Comma-separated states, such as `running,exited`; every state when
    /// absent.
    #[param(example = "running,exited")]
    state: Option<String>,
}

impl ContainersQuery {
    fn filter(self) -> Result<ContainerFilter, ApiError> {
        Ok(ContainerFilter {
            search: self.search.unwrap_or_default(),
            states: self
                .state
                .as_deref()
                .unwrap_or_default()
                .split(',')
                .filter(|name| !name.trim().is_empty())
                .map(state)
                .collect::<Result<_, _>>()?,
        })
    }
}

/// The engines the host agent is configured with, each with its version and
/// figures or why it does not answer.
#[utoipa::path(get, path = "/api/v1/container-engines", params(QueryHeaders),
    responses((status = 200, body = ContainerEngineList)), tag = "containers")]
pub(crate) async fn list_engines<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<ContainerEngineList>, ApiError> {
    let engines = state.containers.engines(request_scope(&headers)?).await?;
    Ok(Json(ContainerEngineList {
        engines: engines.into_iter().map(Into::into).collect(),
    }))
}

async fn set_engine<U>(
    state: ApiState<U>,
    name: String,
    headers: HeaderMap,
    enabled: bool,
) -> Result<Json<ContainerEngineView>, ApiError> {
    let engine = state
        .containers
        .set_engine(command_context(&headers)?, engine(name)?, enabled)
        .await?;
    Ok(Json(engine.into()))
}

/// Enables an engine, so the panel acts on what runs on it.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/enable",
    params(("engine" = String, Path, description = "docker or podman"), MutationHeaders),
    responses((status = 200, body = ContainerEngineView)), tag = "containers")]
pub(crate) async fn enable_engine<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ContainerEngineView>, ApiError> {
    set_engine(state, name, headers, true).await
}

/// Disables an engine; the panel leaves what runs on it alone.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/disable",
    params(("engine" = String, Path, description = "docker or podman"), MutationHeaders),
    responses((status = 200, body = ContainerEngineView)), tag = "containers")]
pub(crate) async fn disable_engine<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ContainerEngineView>, ApiError> {
    set_engine(state, name, headers, false).await
}

/// An enabled engine's containers, searched and filtered by state.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/containers",
    params(("engine" = String, Path, description = "docker or podman"), ContainersQuery, QueryHeaders),
    responses((status = 200, body = ContainerListView)), tag = "containers")]
pub(crate) async fn list_containers<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    Query(query): Query<ContainersQuery>,
    headers: HeaderMap,
) -> Result<Json<ContainerListView>, ApiError> {
    let filter = query.filter()?;
    let containers = state
        .containers
        .containers(request_scope(&headers)?, engine(name)?, filter)
        .await?;
    Ok(Json(containers.into()))
}

/// What may be done to a container besides removing it.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContainerActionName {
    Start,
    /// The container's stop signal, then SIGKILL once its stop timeout
    /// passes.
    Stop,
    Restart,
    /// SIGKILL at once.
    Kill,
}

impl ContainerActionName {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            "kill" => Ok(Self::Kill),
            other => Err(ApiError::new(PanelError::invalid_argument(format!(
                "`{other}` is not start, stop, restart or kill"
            )))),
        }
    }
}

impl From<ContainerActionName> for ContainerAction {
    fn from(value: ContainerActionName) -> Self {
        match value {
            ContainerActionName::Start => Self::Start,
            ContainerActionName::Stop => Self::Stop,
            ContainerActionName::Restart => Self::Restart,
            ContainerActionName::Kill => Self::Kill,
        }
    }
}

/// A container an action was taken on.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerChangeView {
    pub id: String,
    /// Its name before the action.
    pub name: String,
    /// The container afterwards; absent once removed.
    pub container: Option<ContainerView>,
}

impl From<ContainerChange> for ContainerChangeView {
    fn from(value: ContainerChange) -> Self {
        Self {
            id: value.id,
            name: value.name,
            container: value.container.map(Into::into),
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RemoveQuery {
    /// Kills and removes a running container instead of refusing to.
    force: Option<bool>,
    /// Removes its anonymous volumes with it.
    volumes: Option<bool>,
}

/// Starts, stops, restarts or kills a container. The panel's own
/// installation is only ever started, and the audit trail records each
/// action, refused or not.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/containers/{container}/{action}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        ("action" = ContainerActionName, Path, description = "start, stop, restart or kill"),
        MutationHeaders,
    ),
    responses((status = 200, body = ContainerChangeView)), tag = "containers")]
pub(crate) async fn act_on_container<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference, action)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<ContainerChangeView>, ApiError> {
    let action = ContainerActionName::parse(&action)?;
    let change = state
        .containers
        .act(
            command_context(&headers)?,
            engine(name)?,
            container(reference)?,
            action.into(),
        )
        .await?;
    Ok(Json(change.into()))
}

/// Removes a container; a running one only with `force`. The panel's own
/// installation is never removed, and the audit trail records each removal,
/// refused or not.
#[utoipa::path(delete, path = "/api/v1/container-engines/{engine}/containers/{container}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        RemoveQuery,
        MutationHeaders,
    ),
    responses((status = 200, body = ContainerChangeView)), tag = "containers")]
pub(crate) async fn remove_container<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    Query(query): Query<RemoveQuery>,
    headers: HeaderMap,
) -> Result<Json<ContainerChangeView>, ApiError> {
    let action = ContainerAction::Remove {
        force: query.force.unwrap_or(false),
        volumes: query.volumes.unwrap_or(false),
    };
    let change = state
        .containers
        .act(
            command_context(&headers)?,
            engine(name)?,
            container(reference)?,
            action,
        )
        .await?;
    Ok(Json(change.into()))
}

/// Where a container's storage comes from.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerMountView {
    /// `volume`, `bind`, `tmpfs` and so on.
    pub kind: String,
    /// The volume's name, for a volume.
    pub name: Option<String>,
    /// Where it comes from on the host.
    pub source: String,
    /// Where it appears in the container.
    pub destination: String,
    pub read_write: bool,
}

impl From<ContainerMount> for ContainerMountView {
    fn from(value: ContainerMount) -> Self {
        Self {
            kind: value.kind,
            name: value.name,
            source: value.source,
            destination: value.destination,
            read_write: value.read_write,
        }
    }
}

/// A network a container is attached to.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerNetworkView {
    pub name: String,
    pub ip_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub gateway: Option<String>,
    pub mac_address: Option<String>,
    pub aliases: Vec<String>,
}

impl From<ContainerNetwork> for ContainerNetworkView {
    fn from(value: ContainerNetwork) -> Self {
        Self {
            name: value.name,
            ip_address: value.ip_address,
            ipv6_address: value.ipv6_address,
            gateway: value.gateway,
            mac_address: value.mac_address,
            aliases: value.aliases,
        }
    }
}

/// What inspecting a container shows, without its environment or command
/// line, which carry secrets.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerDetailView {
    pub container: ContainerView,
    /// When it last started, RFC 3339.
    pub started_at: Option<String>,
    /// When it last stopped, RFC 3339; absent until it has.
    pub finished_at: Option<String>,
    /// How it last stopped; absent until it has.
    pub exit_code: Option<i64>,
    /// Why it last failed, in the engine's words.
    pub error: Option<String>,
    /// Whether the kernel killed it for running out of memory.
    pub oom_killed: bool,
    /// How often the engine restarted it under its restart policy.
    pub restarts: u32,
    /// `healthy`, `unhealthy` or `starting`; absent without a health check.
    pub health: Option<String>,
    /// `no`, `always`, `unless-stopped` or `on-failure`.
    pub restart_policy: Option<String>,
    /// How often `on-failure` restarts it; 0 for no limit.
    pub restart_retries: u32,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub working_directory: Option<String>,
    /// Such as `linux`.
    pub platform: Option<String>,
    pub mounts: Vec<ContainerMountView>,
    /// By name.
    pub networks: Vec<ContainerNetworkView>,
}

impl From<ContainerDetail> for ContainerDetailView {
    fn from(value: ContainerDetail) -> Self {
        Self {
            container: value.container.into(),
            started_at: value.started_at.map(rfc3339),
            exit_code: value.finished_at.map(|_| value.exit_code),
            finished_at: value.finished_at.map(rfc3339),
            error: value.error,
            oom_killed: value.oom_killed,
            restarts: value.restarts,
            health: value.health,
            restart_policy: value.restart_policy,
            restart_retries: value.restart_retries,
            hostname: value.hostname,
            user: value.user,
            working_directory: value.working_directory,
            platform: value.platform,
            mounts: value.mounts.into_iter().map(Into::into).collect(),
            networks: value.networks.into_iter().map(Into::into).collect(),
        }
    }
}

/// A container's configuration and state, without its environment or
/// command line.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/containers/{container}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        QueryHeaders,
    ),
    responses((status = 200, body = ContainerDetailView)), tag = "containers")]
pub(crate) async fn inspect_container<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ContainerDetailView>, ApiError> {
    let detail = state
        .containers
        .inspect(
            request_scope(&headers)?,
            engine(name)?,
            container(reference)?,
        )
        .await?;
    Ok(Json(detail.into()))
}

/// Which of a container's outputs a line came from.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContainerLogStreamName {
    /// Standard output, and everything a container with a terminal prints.
    Stdout,
    Stderr,
}

/// A line a container printed.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerLogLineView {
    /// When its engine recorded it, RFC 3339 with up to nanoseconds.
    pub time: String,
    pub stream: ContainerLogStreamName,
    /// Without its line break; cut at 16 KiB.
    pub text: String,
}

impl From<ContainerLogLine> for ContainerLogLineView {
    fn from(value: ContainerLogLine) -> Self {
        Self {
            time: precise(value.time),
            stream: match value.stream {
                ContainerLogStream::Stdout => ContainerLogStreamName::Stdout,
                ContainerLogStream::Stderr => ContainerLogStreamName::Stderr,
            },
            text: value.text,
        }
    }
}

/// A container's last lines.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerLogsView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// Oldest first.
    pub lines: Vec<ContainerLogLineView>,
    /// Older lines were left out to keep the answer within 2 MiB.
    pub truncated: bool,
}

impl From<ContainerLogs> for ContainerLogsView {
    fn from(value: ContainerLogs) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            lines: value.lines.into_iter().map(Into::into).collect(),
            truncated: value.truncated,
        }
    }
}

/// How many lines a query asks for, between 1 and `most`.
fn lines(value: Option<u32>, default: u32, least: u32, most: u32) -> Result<u32, ApiError> {
    let lines = value.unwrap_or(default);
    if (least..=most).contains(&lines) {
        Ok(lines)
    } else {
        Err(ApiError::new(PanelError::invalid_argument(format!(
            "lines must be between {least} and {most}"
        ))))
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContainerLogsQuery {
    /// The last this many lines, 1 to 5000; 200 by default.
    #[param(minimum = 1, maximum = 5000)]
    lines: Option<u32>,
    /// RFC 3339: only lines at or after this time.
    since: Option<String>,
}

/// The last lines a container printed, oldest first, as its engine's log
/// driver kept them. What a container prints can hold secrets.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/containers/{container}/logs",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        ContainerLogsQuery,
        QueryHeaders,
    ),
    responses((status = 200, body = ContainerLogsView)), tag = "containers")]
pub(crate) async fn container_logs<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    Query(query): Query<ContainerLogsQuery>,
    headers: HeaderMap,
) -> Result<Json<ContainerLogsView>, ApiError> {
    let query = ContainerLogQuery {
        lines: lines(query.lines, DEFAULT_LINES, 1, MOST_LINES)?,
        since: parse_time("since", query.since.as_deref())?,
    };
    let logs = state
        .containers
        .logs(
            request_scope(&headers)?,
            engine(name)?,
            container(reference)?,
            query,
        )
        .await?;
    Ok(Json(logs.into()))
}

/// What following a container sends: lines oldest first and where to
/// resume after them, or why following ended.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerLogTailMessage {
    pub lines: Vec<ContainerLogLineView>,
    /// Pass as `after` to resume after these lines.
    pub cursor: Option<String>,
    pub error: Option<LogTailError>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContainerLogTailQuery {
    /// How many of the lines printed before to send first, 0 to 1000; 100
    /// by default.
    #[param(maximum = 1000)]
    lines: Option<u32>,
    /// RFC 3339: resume after the line printed at this time, a message's
    /// `cursor`, instead.
    after: Option<String>,
}

/// The lines as messages, each with where to resume after it.
fn tail_messages(tail: ContainerLogTail) -> impl Stream<Item = Relayed<ContainerLogTailMessage>> {
    tail.scan(None, |cursor, batch| {
        ready(Some(match batch {
            Ok(lines) => {
                if let Some(last) = lines.last() {
                    *cursor = Some(last.time);
                }
                Relayed::Sent(ContainerLogTailMessage {
                    lines: lines.into_iter().map(Into::into).collect(),
                    cursor: cursor.map(precise),
                    error: None,
                })
            }
            Err(error) => Relayed::Failed(
                ContainerLogTailMessage {
                    lines: Vec::new(),
                    cursor: cursor.map(precise),
                    error: Some(LogTailError::from(&error)),
                },
                error,
            ),
        }))
    })
}

/// Follows the lines a container prints as it prints them, over a
/// WebSocket, until the container stops. Each text message is a
/// `ContainerLogTailMessage`; one with an error is the last.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/containers/{container}/logs/tail",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        ContainerLogTailQuery,
        QueryHeaders,
    ),
    responses((status = 101, description = "Switching to the WebSocket protocol")), tag = "containers")]
pub(crate) async fn tail_container_logs<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    Query(query): Query<ContainerLogTailQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let start = match parse_time("after", query.after.as_deref())? {
        Some(after) => ContainerLogStart::After(after),
        None => ContainerLogStart::Last(lines(query.lines, DEFAULT_BACKLOG, 0, MOST_BACKLOG)?),
    };
    let tail = state
        .containers
        .follow_logs(
            request_scope(&headers)?,
            engine(name)?,
            container(reference)?,
            start,
        )
        .await?;
    let stopped = CloseFrame {
        code: 1000,
        reason: Utf8Bytes::from_static("the container stopped"),
    };
    Ok(upgrade.on_upgrade(move |socket| relay(socket, tail_messages(tail), Some(stopped))))
}

/// A container's traffic, summed over its interfaces.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerNetworkStatsView {
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub received_packets: u64,
    pub sent_packets: u64,
    /// Packets received or sent in error.
    pub errors: u64,
    /// Packets dropped on the way in or out.
    pub dropped: u64,
}

impl From<ContainerNetworkStats> for ContainerNetworkStatsView {
    fn from(value: ContainerNetworkStats) -> Self {
        Self {
            received_bytes: value.received_bytes,
            sent_bytes: value.sent_bytes,
            received_packets: value.received_packets,
            sent_packets: value.sent_packets,
            errors: value.errors,
            dropped: value.dropped,
        }
    }
}

/// What a running container uses, read once as `docker stats --no-stream`
/// reads it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerStatsView {
    pub id: String,
    /// Without the leading slash.
    pub name: String,
    /// When the engine read them, RFC 3339.
    pub read_at: Option<String>,
    /// Of one CPU, over about a second: 250 is two and a half CPUs busy.
    pub cpu_percent: f64,
    /// The CPUs the container may use.
    pub online_cpus: u32,
    /// Without the page cache the kernel can reclaim.
    pub memory_bytes: u64,
    /// The host's memory when the container has no limit.
    pub memory_limit_bytes: u64,
    /// Absent for a container without a network of its own, such as one
    /// on the host's.
    pub network: Option<ContainerNetworkStatsView>,
    pub block_read_bytes: u64,
    pub block_written_bytes: u64,
    /// Processes and threads.
    pub pids: u64,
}

impl From<ContainerStats> for ContainerStatsView {
    fn from(value: ContainerStats) -> Self {
        Self {
            id: value.id,
            name: value.name,
            read_at: value.read_at.map(precise),
            cpu_percent: value.cpu_percent,
            online_cpus: value.online_cpus,
            memory_bytes: value.memory_bytes,
            memory_limit_bytes: value.memory_limit_bytes,
            network: value.network.map(Into::into),
            block_read_bytes: value.block_read_bytes,
            block_written_bytes: value.block_written_bytes,
            pids: value.pids,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContainerStatsListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// Every running container, by name.
    pub stats: Vec<ContainerStatsView>,
}

impl From<ContainerStatsList> for ContainerStatsListView {
    fn from(value: ContainerStatsList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            stats: value.stats.into_iter().map(Into::into).collect(),
        }
    }
}

/// What every running container on an enabled engine uses, read at most
/// 16 at a time; each takes the engine about a second.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/stats",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = ContainerStatsListView)), tag = "containers")]
pub(crate) async fn list_container_stats<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ContainerStatsListView>, ApiError> {
    let stats = state
        .containers
        .stats(request_scope(&headers)?, engine(name)?, None)
        .await?;
    Ok(Json(stats.into()))
}

/// What a running container uses.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/containers/{container}/stats",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("container" = String, Path, description = "Its ID, a unique prefix of its ID or its name"),
        QueryHeaders,
    ),
    responses((status = 200, body = ContainerStatsView)), tag = "containers")]
pub(crate) async fn container_stats<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ContainerStatsView>, ApiError> {
    let stats = state
        .containers
        .stats(
            request_scope(&headers)?,
            engine(name)?,
            Some(container(reference)?),
        )
        .await?;
    let one = stats
        .stats
        .into_iter()
        .next()
        .ok_or_else(|| PanelError::internal("the agent sent no statistics"))?;
    Ok(Json(one.into()))
}
