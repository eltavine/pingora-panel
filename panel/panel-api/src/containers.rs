//! The container engines the host agent reaches and what runs on them
//! (ADR 0031).

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    ContainerEngine, ContainerFilter, ContainerList, ContainerState, ContainerSummary, EngineInfo,
    EngineVersion, PortMapping,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

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
