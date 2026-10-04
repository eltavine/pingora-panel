//! What the host agent does for the panel (ADR 0028, ADR 0030).

use crate::{
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{extract::State, http::HeaderMap, Json};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, DirectoriesReport,
    DirectoryKind, DirectoryUsage,
};
use panel_errors::ErrorCode;
use serde::Serialize;
use utoipa::ToSchema;

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
            observed_at: value
                .observed_at
                .map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)),
            directories: value.directories.into_iter().map(Into::into).collect(),
        }
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
