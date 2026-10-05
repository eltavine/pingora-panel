//! The static sites' files (ADR 0034), below the directory the gateway
//! serves them from.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{SiteEntryKind, SitePath, SiteRemoval, WriteCondition, MOST_FILE_BYTES};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use utoipa::{IntoParams, ToSchema};

/// The most a request body to write a file holds, past the file itself.
pub(crate) const MOST_WRITE_BYTES: usize = MOST_FILE_BYTES as usize + 1024;

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A file's bytes, as they are.
#[derive(ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct FileBytes(#[allow(dead_code)] Vec<u8>);

fn path(value: &str) -> Result<SitePath, ApiError> {
    SitePath::parse(value).map_err(ApiError::new)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PathQuery {
    /// Below the sites' directory, such as `shop/index.html`; empty for the
    /// directory itself.
    #[serde(default)]
    path: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RemoveQuery {
    /// Below the sites' directory, such as `shop/old`.
    path: String,
    /// Removes a directory with what it holds; a directory that holds
    /// anything is refused otherwise.
    #[serde(default)]
    recursive: bool,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SiteEntryKindName {
    File,
    Directory,
    /// A symbolic link, never followed out of the sites' directory.
    Link,
    Other,
}

impl From<SiteEntryKind> for SiteEntryKindName {
    fn from(value: SiteEntryKind) -> Self {
        match value {
            SiteEntryKind::File => Self::File,
            SiteEntryKind::Directory => Self::Directory,
            SiteEntryKind::Link => Self::Link,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteEntryView {
    pub name: String,
    pub kind: SiteEntryKindName,
    /// A file's size; 0 for anything else.
    pub size_bytes: u64,
    /// RFC 3339.
    pub modified: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteDirectoryView {
    /// Below the sites' directory; empty for the directory itself.
    pub path: String,
    /// Directories first, then by name.
    pub entries: Vec<SiteEntryView>,
}

/// A directory below the sites' directory, the directory itself when no
/// path is named.
#[utoipa::path(get, path = "/api/v1/site-files", params(PathQuery, QueryHeaders),
    responses((status = 200, body = SiteDirectoryView)), tag = "files")]
pub(crate) async fn list_directory<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Json<SiteDirectoryView>, ApiError> {
    let directory = state
        .site_files
        .directory(request_scope(&headers)?, path(&query.path)?)
        .await?;
    Ok(Json(SiteDirectoryView {
        path: directory.path.as_str().to_owned(),
        entries: directory
            .entries
            .into_iter()
            .map(|entry| SiteEntryView {
                name: entry.name,
                kind: entry.kind.into(),
                size_bytes: entry.size_bytes,
                modified: entry.modified.map(rfc3339),
            })
            .collect(),
    }))
}

/// A file whole, up to 64 MiB, as an attachment: it is never shown as a
/// page of the console's origin.
#[utoipa::path(get, path = "/api/v1/site-files/content", params(PathQuery, QueryHeaders),
    responses((status = 200, description = "The file's bytes; its entity tag in `ETag`",
        content_type = "application/octet-stream", body = FileBytes)),
    tag = "files")]
pub(crate) async fn read_file<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let file = state
        .site_files
        .file(request_scope(&headers)?, path(&query.path)?)
        .await?;
    let name = file.path.name().unwrap_or("file").replace(['"', '\\'], "_");
    let mut response = file.content.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    if let Ok(tag) = HeaderValue::from_str(&file.tag) {
        headers.insert(header::ETAG, tag);
    }
    if let Some(modified) = file.modified {
        if let Ok(value) = HeaderValue::from_str(&httpdate::fmt_http_date(modified)) {
            headers.insert(header::LAST_MODIFIED, value);
        }
    }
    Ok(response)
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteFileWrittenView {
    pub path: String,
    pub size_bytes: u64,
    /// The content's SHA-256, in hexadecimal.
    pub sha256: String,
    /// Whether the file was new.
    pub created: bool,
}

/// What `If-Match` and `If-None-Match` ask of a write (RFC 9110 §13.1).
fn condition(headers: &HeaderMap) -> Result<WriteCondition, ApiError> {
    let text = |name: header::HeaderName| {
        headers
            .get(&name)
            .map(|value| {
                value.to_str().map(str::trim).map_err(|_| {
                    ApiError::new(PanelError::invalid_argument(format!(
                        "{name} must be visible ASCII"
                    )))
                })
            })
            .transpose()
    };
    match (text(header::IF_MATCH)?, text(header::IF_NONE_MATCH)?) {
        (Some(_), Some(_)) => Err(ApiError::new(PanelError::invalid_argument(
            "send If-Match or If-None-Match, not both",
        ))),
        (Some("*"), None) | (None, None) => Ok(WriteCondition::Any),
        (Some(tag), None) => Ok(WriteCondition::Tagged(tag.to_owned())),
        (None, Some("*")) => Ok(WriteCondition::Absent),
        (None, Some(_)) => Err(ApiError::new(PanelError::invalid_argument(
            "If-None-Match takes only *",
        ))),
    }
}

/// Writes a file whole, creating it and its directories or replacing it
/// atomically. `If-Match` replaces only the file of that entity tag and
/// `If-None-Match: *` only creates one. The audit trail records each write,
/// refused or not, with its size and digest.
#[utoipa::path(put, path = "/api/v1/site-files/content", params(PathQuery, MutationHeaders),
    request_body(content = FileBytes, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Replaced", body = SiteFileWrittenView),
        (status = 201, description = "Created", body = SiteFileWrittenView),
    ),
    tag = "files")]
pub(crate) async fn write_file<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let condition = condition(&headers)?;
    let written = state
        .site_files
        .write_file(
            command_context(&headers)?,
            path(&query.path)?,
            body.to_vec(),
            condition,
        )
        .await?;
    let status = if written.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let tag = HeaderValue::from_str(&written.tag).ok();
    let mut response = (
        status,
        Json(SiteFileWrittenView {
            path: written.path.as_str().to_owned(),
            size_bytes: written.size_bytes,
            sha256: written.sha256,
            created: written.created,
        }),
    )
        .into_response();
    if let Some(tag) = tag {
        response.headers_mut().insert(header::ETAG, tag);
    }
    Ok(response)
}

/// Creates a directory and the directories above it.
#[utoipa::path(post, path = "/api/v1/site-files/directories", params(PathQuery, MutationHeaders),
    responses((status = 204, description = "Created, or there already")), tag = "files")]
pub(crate) async fn create_directory<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state
        .site_files
        .create_directory(command_context(&headers)?, path(&query.path)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SiteRemovalView {
    pub path: String,
    pub kind: SiteEntryKindName,
    /// The files and directories removed, the entry itself included.
    pub removed: u64,
}

impl From<SiteRemoval> for SiteRemovalView {
    fn from(value: SiteRemoval) -> Self {
        Self {
            path: value.path.as_str().to_owned(),
            kind: value.kind.into(),
            removed: value.removed,
        }
    }
}

/// Removes a file, a link or a directory: an empty one, or with what it
/// holds when `recursive`.
#[utoipa::path(delete, path = "/api/v1/site-files", params(RemoveQuery, MutationHeaders),
    responses((status = 200, body = SiteRemovalView)), tag = "files")]
pub(crate) async fn remove_entry<U>(
    State(state): State<ApiState<U>>,
    Query(query): Query<RemoveQuery>,
    headers: HeaderMap,
) -> Result<Json<SiteRemovalView>, ApiError> {
    let removal = state
        .site_files
        .remove(
            command_context(&headers)?,
            path(&query.path)?,
            query.recursive,
        )
        .await?;
    Ok(Json(removal.into()))
}
