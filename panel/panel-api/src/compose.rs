//! The Compose projects on the container engines the host agent reaches
//! (ADR 0031), known by the labels Compose puts on their containers.

use crate::{
    containers::{engine, lines, ContainerLogLineView, DEFAULT_LINES, MOST_LINES},
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    tail::LogTailError,
    time::parse_time,
    ApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    ComposeAction, ComposeChange, ComposeFile, ComposeLogs, ComposeProject, ComposeProjectList,
    ContainerLogQuery,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use utoipa::{IntoParams, ToSchema};

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A project's name as Compose allows it: lowercase letters, digits,
/// dashes and underscores, starting with a letter or digit.
fn project(name: String) -> Result<String, ApiError> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if valid {
        Ok(name)
    } else {
        Err(ApiError::new(PanelError::invalid_argument(format!(
            "`{name}` is not a Compose project's name"
        ))))
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ProjectServiceView {
    pub name: String,
    pub containers: u32,
    pub running: u32,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeProjectView {
    pub name: String,
    /// Where Compose ran, on the host.
    pub working_directory: Option<String>,
    /// The files Compose read, on the host.
    pub config_files: Vec<String>,
    /// By name.
    pub services: Vec<ProjectServiceView>,
    pub containers: u32,
    pub running: u32,
    /// The panel's own installation, which is only ever brought up.
    pub installation: bool,
}

impl From<ComposeProject> for ComposeProjectView {
    fn from(value: ComposeProject) -> Self {
        Self {
            name: value.name,
            working_directory: value.working_directory,
            config_files: value.config_files,
            services: value
                .services
                .into_iter()
                .map(|service| ProjectServiceView {
                    name: service.name,
                    containers: service.containers,
                    running: service.running,
                })
                .collect(),
            containers: value.containers,
            running: value.running,
            installation: value.installation,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeProjectListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// By name.
    pub projects: Vec<ComposeProjectView>,
}

impl From<ComposeProjectList> for ComposeProjectListView {
    fn from(value: ComposeProjectList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            projects: value.projects.into_iter().map(Into::into).collect(),
        }
    }
}

/// An enabled engine's Compose projects, with their services and how many
/// of their containers run.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/compose-projects",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = ComposeProjectListView)), tag = "containers")]
pub(crate) async fn list_projects<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ComposeProjectListView>, ApiError> {
    let projects = state
        .compose
        .projects(request_scope(&headers)?, engine(name)?)
        .await?;
    Ok(Json(projects.into()))
}

/// What may be done to a Compose project.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ComposeActionName {
    /// Starts the containers that are not running.
    Up,
    /// Stops and removes the containers and the project's networks, and
    /// keeps its volumes.
    Down,
    Restart,
}

impl ComposeActionName {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "restart" => Ok(Self::Restart),
            other => Err(ApiError::new(PanelError::invalid_argument(format!(
                "`{other}` is not up, down or restart"
            )))),
        }
    }
}

impl From<ComposeActionName> for ComposeAction {
    fn from(value: ComposeActionName) -> Self {
        match value {
            ComposeActionName::Up => Self::Up,
            ComposeActionName::Down => Self::Down,
            ComposeActionName::Restart => Self::Restart,
        }
    }
}

/// What the engine refused, when it refused only some of an action.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeFailureView {
    /// A container's or network's name.
    pub name: String,
    pub error: LogTailError,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeChangeView {
    /// The project afterwards; absent once it is down.
    pub project: Option<ComposeProjectView>,
    /// How many containers the action changed.
    pub changed: u32,
    pub failures: Vec<ComposeFailureView>,
}

impl From<ComposeChange> for ComposeChangeView {
    fn from(value: ComposeChange) -> Self {
        Self {
            project: value.project.map(Into::into),
            changed: value.changed,
            failures: value
                .failures
                .into_iter()
                .map(|failure| ComposeFailureView {
                    error: LogTailError::from(&failure.error),
                    name: failure.name,
                })
                .collect(),
        }
    }
}

/// Brings a Compose project up, down or restarts it. The panel's own
/// installation is only ever brought up, and the audit trail records each
/// action, refused or not.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/compose-projects/{project}/{action}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("project" = String, Path, description = "The project's name"),
        ("action" = ComposeActionName, Path, description = "up, down or restart"),
        MutationHeaders,
    ),
    responses((status = 200, body = ComposeChangeView)), tag = "containers")]
pub(crate) async fn act_on_project<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference, action)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<ComposeChangeView>, ApiError> {
    let action = ComposeActionName::parse(&action)?;
    let change = state
        .compose
        .act_on_project(
            command_context(&headers)?,
            engine(name)?,
            project(reference)?,
            action.into(),
        )
        .await?;
    Ok(Json(change.into()))
}

/// A line one of a project's containers printed.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeLogLineView {
    pub service: String,
    /// Without the leading slash.
    pub container: String,
    pub line: ContainerLogLineView,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeLogsView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// Oldest first.
    pub lines: Vec<ComposeLogLineView>,
    /// Older lines were left out to keep the answer within 2 MiB.
    pub truncated: bool,
}

impl From<ComposeLogs> for ComposeLogsView {
    fn from(value: ComposeLogs) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            lines: value
                .lines
                .into_iter()
                .map(|line| ComposeLogLineView {
                    service: line.service,
                    container: line.container,
                    line: line.line.into(),
                })
                .collect(),
            truncated: value.truncated,
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ProjectLogsQuery {
    /// The last this many lines of all its containers together, 1 to 5000;
    /// 200 by default.
    #[param(minimum = 1, maximum = 5000)]
    lines: Option<u32>,
    /// RFC 3339: only lines at or after this time.
    since: Option<String>,
}

/// The last lines a project's containers printed, merged by time, each
/// with its service. What containers print can hold secrets.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/compose-projects/{project}/logs",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("project" = String, Path, description = "The project's name"),
        ProjectLogsQuery,
        QueryHeaders,
    ),
    responses((status = 200, body = ComposeLogsView)), tag = "containers")]
pub(crate) async fn project_logs<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    Query(query): Query<ProjectLogsQuery>,
    headers: HeaderMap,
) -> Result<Json<ComposeLogsView>, ApiError> {
    let query = ContainerLogQuery {
        lines: lines(query.lines, DEFAULT_LINES, 1, MOST_LINES)?,
        since: parse_time("since", query.since.as_deref())?,
    };
    let logs = state
        .compose
        .project_logs(
            request_scope(&headers)?,
            engine(name)?,
            project(reference)?,
            query,
        )
        .await?;
    Ok(Json(logs.into()))
}

/// A Compose file a project's labels name.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeFileView {
    /// On the host.
    pub path: String,
    /// Absent when it could not be read.
    pub content: Option<String>,
    /// Why it could not be read.
    pub error: Option<LogTailError>,
}

impl From<ComposeFile> for ComposeFileView {
    fn from(value: ComposeFile) -> Self {
        let (content, error) = match value.content {
            Ok(content) => (Some(content), None),
            Err(error) => (None, Some(LogTailError::from(&error))),
        };
        Self {
            path: value.path,
            content,
            error,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ComposeFilesView {
    /// In the order Compose read them.
    pub files: Vec<ComposeFileView>,
}

/// The Compose files a project's labels name, read only when each lies in
/// the project's working directory, ends in `.yml` or `.yaml`, is a
/// regular file and is at most 256 KiB. They can hold secrets.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/compose-projects/{project}/files",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("project" = String, Path, description = "The project's name"),
        QueryHeaders,
    ),
    responses((status = 200, body = ComposeFilesView)), tag = "containers")]
pub(crate) async fn project_files<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ComposeFilesView>, ApiError> {
    let files = state
        .compose
        .project_files(request_scope(&headers)?, engine(name)?, project(reference)?)
        .await?;
    Ok(Json(ComposeFilesView {
        files: files.into_iter().map(Into::into).collect(),
    }))
}
