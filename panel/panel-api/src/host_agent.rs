//! What the host agent does for the panel (ADR 0028, ADR 0030).

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
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, DirectoriesReport,
    DirectoryKind, DirectoryUsage, GatewayUnitStatus, ListenersReport, ListeningProcess,
    PortListener, UnitAction,
};
use panel_errors::{ErrorCode, PanelError};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use utoipa::{IntoParams, ToSchema};

/// How many ports one look at listeners may name.
const MAX_PORTS: usize = 16;

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Whether the panel reaches a host agent.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentStatusName {
    Connected,
    /// The installation has no agent; host actions are not offered.
    NotConfigured,
    /// An agent is configured but does not answer.
    Unreachable,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentCapabilityName {
    Directories,
    Listeners,
    GatewayUnit,
    Containers,
}

impl From<AgentCapability> for AgentCapabilityName {
    fn from(value: AgentCapability) -> Self {
        match value {
            AgentCapability::Directories => Self::Directories,
            AgentCapability::Listeners => Self::Listeners,
            AgentCapability::GatewayUnit => Self::GatewayUnit,
            AgentCapability::Containers => Self::Containers,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CapabilityStateName {
    Available,
    /// The operator has not enabled it on the agent.
    NotEnabled,
    /// The host's platform cannot provide it.
    Unsupported,
    /// Enabled, but the agent lacks the privilege it needs.
    Denied,
    /// Enabled, but what it relies on does not answer.
    Unreachable,
}

impl From<CapabilityState> for CapabilityStateName {
    fn from(value: CapabilityState) -> Self {
        match value {
            CapabilityState::Available => Self::Available,
            CapabilityState::NotEnabled => Self::NotEnabled,
            CapabilityState::Unsupported => Self::Unsupported,
            CapabilityState::Denied => Self::Denied,
            CapabilityState::Unreachable => Self::Unreachable,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentCapabilityView {
    pub capability: AgentCapabilityName,
    pub state: CapabilityStateName,
    /// What to do about a state other than available.
    pub detail: String,
}

impl From<CapabilityStatus> for AgentCapabilityView {
    fn from(value: CapabilityStatus) -> Self {
        Self {
            capability: value.capability.into(),
            state: value.state.into(),
            detail: value.detail,
        }
    }
}

/// The host agent and what it can do now.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HostAgentView {
    pub status: AgentStatusName,
    /// The agent's version, when connected.
    pub build: Option<String>,
    pub hostname: Option<String>,
    pub capabilities: Vec<AgentCapabilityView>,
}

impl HostAgentView {
    fn without(status: AgentStatusName) -> Self {
        Self {
            status,
            build: None,
            hostname: None,
            capabilities: Vec::new(),
        }
    }
}

impl From<AgentDescription> for HostAgentView {
    fn from(value: AgentDescription) -> Self {
        Self {
            status: AgentStatusName::Connected,
            build: Some(value.build),
            hostname: (!value.hostname.is_empty()).then_some(value.hostname),
            capabilities: value.capabilities.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DirectoryKindName {
    /// The gateway's configuration and state.
    Configuration,
    Logs,
    /// Certificates and their keys.
    Certificates,
}

impl From<DirectoryKind> for DirectoryKindName {
    fn from(value: DirectoryKind) -> Self {
        match value {
            DirectoryKind::Configuration => Self::Configuration,
            DirectoryKind::Logs => Self::Logs,
            DirectoryKind::Certificates => Self::Certificates,
        }
    }
}

/// The space one of the panel's directories takes on the host.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DirectoryUsageView {
    pub kind: DirectoryKindName,
    pub path: String,
    /// False when the directory does not exist.
    pub present: bool,
    /// The sizes of the regular files below it.
    pub bytes: u64,
    pub files: u64,
    /// Entries the agent could not read, and so did not count.
    pub unreadable: u64,
    /// True when the agent stopped at its limit and the counts are partial.
    pub truncated: bool,
}

impl From<DirectoryUsage> for DirectoryUsageView {
    fn from(value: DirectoryUsage) -> Self {
        Self {
            kind: value.kind.into(),
            path: value.path,
            present: value.present,
            bytes: value.bytes,
            files: value.files,
            unreadable: value.unreadable,
            truncated: value.truncated,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DirectoriesView {
    /// When the agent measured them, RFC 3339.
    pub observed_at: Option<String>,
    pub directories: Vec<DirectoryUsageView>,
}

impl From<DirectoriesReport> for DirectoriesView {
    fn from(value: DirectoriesReport) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            directories: value.directories.into_iter().map(Into::into).collect(),
        }
    }
}

/// A process that holds a listening socket.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ListeningProcessView {
    pub pid: i32,
    /// The command's name, such as `nginx`.
    pub name: String,
    /// The executable's path, when the agent may read it.
    pub executable: Option<String>,
    pub uid: u32,
}

impl From<ListeningProcess> for ListeningProcessView {
    fn from(value: ListeningProcess) -> Self {
        Self {
            pid: value.pid,
            name: value.name,
            executable: (!value.executable.is_empty()).then_some(value.executable),
            uid: value.uid,
        }
    }
}

/// A socket listening on a TCP port, with the processes that hold it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PortListenerView {
    /// The local address, such as `0.0.0.0` or `::`.
    pub address: String,
    pub port: u16,
    /// The socket's owner.
    pub uid: u32,
    /// Empty when the agent may not see them.
    pub processes: Vec<ListeningProcessView>,
}

impl From<PortListener> for PortListenerView {
    fn from(value: PortListener) -> Self {
        Self {
            address: value.address,
            port: value.port,
            uid: value.uid,
            processes: value.processes.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ListenersView {
    /// When the agent looked, RFC 3339.
    pub observed_at: Option<String>,
    /// By port, then address; empty when nothing listens.
    pub listeners: Vec<PortListenerView>,
}

impl From<ListenersReport> for ListenersView {
    fn from(value: ListenersReport) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            listeners: value.listeners.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListenersQuery {
    /// Comma-separated TCP ports, at most 16; 80 and 443 when absent.
    #[param(example = "80,443")]
    ports: Option<String>,
}

impl ListenersQuery {
    fn ports(&self) -> Result<Vec<u16>, ApiError> {
        let Some(ports) = self
            .ports
            .as_deref()
            .filter(|ports| !ports.trim().is_empty())
        else {
            return Ok(Vec::new());
        };
        let ports = ports
            .split(',')
            .map(|port| {
                port.trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| {
                        ApiError::new(PanelError::invalid_argument(format!(
                            "`{}` is not a TCP port",
                            port.trim()
                        )))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if ports.len() > MAX_PORTS {
            return Err(ApiError::new(PanelError::invalid_argument(format!(
                "name at most {MAX_PORTS} ports"
            ))));
        }
        Ok(ports)
    }
}

/// Whether the panel reaches a host agent and which of its capabilities
/// are available.
#[utoipa::path(get, path = "/api/v1/host/agent", params(QueryHeaders),
    responses((status = 200, body = HostAgentView)), tag = "host")]
pub(crate) async fn host_agent<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<HostAgentView>, ApiError> {
    match state.host_agent.agent(request_scope(&headers)?).await {
        Ok(description) => Ok(Json(description.into())),
        Err(error) if error.code.as_str() == ErrorCode::UNSUPPORTED_CAPABILITY => {
            Ok(Json(HostAgentView::without(AgentStatusName::NotConfigured)))
        }
        Err(error) if error.retryable || error.code.as_str() == ErrorCode::UNAVAILABLE => {
            tracing::warn!(error_code = %error.code, error = %error.message, "host agent unreachable");
            Ok(Json(HostAgentView::without(AgentStatusName::Unreachable)))
        }
        Err(error) => Err(error.into()),
    }
}

/// The space the panel's configuration, log and certificate directories
/// take on the host.
#[utoipa::path(get, path = "/api/v1/host/directories", params(QueryHeaders),
    responses((status = 200, body = DirectoriesView)), tag = "host")]
pub(crate) async fn host_directories<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<DirectoriesView>, ApiError> {
    let report = state
        .host_agent
        .directories(request_scope(&headers)?)
        .await?;
    Ok(Json(report.into()))
}

/// Which processes listen on TCP ports, such as whatever holds 80 and 443
/// before the gateway can.
#[utoipa::path(get, path = "/api/v1/host/listeners", params(ListenersQuery, QueryHeaders),
    responses((status = 200, body = ListenersView)), tag = "host")]
pub(crate) async fn host_listeners<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<ListenersQuery>,
    headers: HeaderMap,
) -> Result<Json<ListenersView>, ApiError> {
    let ports = query.ports()?;
    let report = state
        .host_agent
        .listeners(request_scope(&headers)?, ports)
        .await?;
    Ok(Json(report.into()))
}

/// What may be done to the gateway's unit.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnitActionName {
    Start,
    Stop,
    Restart,
}

impl UnitActionName {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            other => Err(ApiError::new(PanelError::invalid_argument(format!(
                "`{other}` is not start, stop or restart"
            )))),
        }
    }
}

impl From<UnitActionName> for UnitAction {
    fn from(value: UnitActionName) -> Self {
        match value {
            UnitActionName::Start => Self::Start,
            UnitActionName::Stop => Self::Stop,
            UnitActionName::Restart => Self::Restart,
        }
    }
}

/// The gateway's systemd unit, in systemd's own words.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GatewayUnitView {
    /// Such as `pingora-panel-gatewayd.service`.
    pub name: String,
    pub description: String,
    /// Such as `loaded` or `not-found`.
    pub load_state: String,
    /// Such as `active`, `inactive` or `failed`.
    pub active_state: String,
    /// Such as `running` or `dead`.
    pub sub_state: String,
    /// Such as `enabled` or `disabled`.
    pub unit_file_state: String,
    /// The main process, when there is one.
    pub main_pid: Option<u32>,
    /// When it last became active, RFC 3339.
    pub active_since: Option<String>,
    /// How often systemd restarted it on its own.
    pub restarts: u32,
    /// How it last ended, such as `success` or `exit-code`.
    pub result: String,
}

impl From<GatewayUnitStatus> for GatewayUnitView {
    fn from(value: GatewayUnitStatus) -> Self {
        Self {
            name: value.name,
            description: value.description,
            load_state: value.load_state,
            active_state: value.active_state,
            sub_state: value.sub_state,
            unit_file_state: value.unit_file_state,
            main_pid: (value.main_pid != 0).then_some(value.main_pid),
            active_since: value.active_since.map(rfc3339),
            restarts: value.restarts,
            result: value.result,
        }
    }
}

/// The gateway's systemd unit, when the host agent manages it.
#[utoipa::path(get, path = "/api/v1/host/gateway-unit", params(QueryHeaders),
    responses((status = 200, body = GatewayUnitView)), tag = "host")]
pub(crate) async fn gateway_unit<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<GatewayUnitView>, ApiError> {
    let status = state
        .host_agent
        .gateway_unit(request_scope(&headers)?)
        .await?;
    Ok(Json(status.into()))
}

/// Starts, stops or restarts the gateway's unit and answers once systemd
/// has finished; the audit trail records it, refused or not.
#[utoipa::path(post, path = "/api/v1/host/gateway-unit/{action}",
    params(("action" = UnitActionName, Path, description = "start, stop or restart"), MutationHeaders),
    responses((status = 200, body = GatewayUnitView)), tag = "host")]
pub(crate) async fn change_gateway_unit<U>(
    State(state): State<ApiState<U>>,
    Path(action): Path<String>,
    headers: HeaderMap,
) -> Result<Json<GatewayUnitView>, ApiError> {
    let action = UnitActionName::parse(&action)?;
    let status = state
        .host_agent
        .change_gateway_unit(command_context(&headers)?, action.into())
        .await?;
    Ok(Json(status.into()))
}
