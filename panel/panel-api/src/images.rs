//! The images on the container engines the host agent reaches (ADR 0031).

use crate::{
    containers::engine,
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
use panel_application::{Image, ImageDetail, ImageList, ImageRemoval};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// An image as a path names it: its ID, a prefix of its ID or a reference
/// such as `ghcr.io/example/app:2.3`, with its slashes percent-encoded. Each
/// part between slashes is a name, never `.` or `..`.
fn image(reference: String) -> Result<String, ApiError> {
    let valid = !reference.is_empty()
        && reference.len() <= 512
        && reference.starts_with(|c: char| c.is_ascii_alphanumeric())
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'))
        && reference
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if valid {
        Ok(reference)
    } else {
        Err(ApiError::new(PanelError::invalid_argument(format!(
            "`{reference}` is not an image's ID or reference"
        ))))
    }
}

/// An image as a list shows it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ImageView {
    /// Such as `sha256:4f1c…`.
    pub id: String,
    /// Such as `nginx:1.27`; empty for an image nothing names any more.
    pub tags: Vec<String>,
    /// Such as `nginx@sha256:…`.
    pub digests: Vec<String>,
    /// RFC 3339.
    pub created: Option<String>,
    /// Layers it shares with other images included.
    pub size_bytes: u64,
    /// The containers, running or not, created from it.
    pub containers: u32,
    pub labels: BTreeMap<String, String>,
}

impl From<Image> for ImageView {
    fn from(value: Image) -> Self {
        Self {
            id: value.id,
            tags: value.tags,
            digests: value.digests,
            created: value.created.map(rfc3339),
            size_bytes: value.size_bytes,
            containers: value.containers,
            labels: value.labels,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ImageListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// By tag; images nothing names last.
    pub images: Vec<ImageView>,
}

impl From<ImageList> for ImageListView {
    fn from(value: ImageList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            images: value.images.into_iter().map(Into::into).collect(),
        }
    }
}

/// An image's configuration, without its environment or command line,
/// which can carry secrets.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ImageDetailView {
    pub image: ImageView,
    /// Such as `amd64` or `arm64`.
    pub architecture: Option<String>,
    /// Such as `v8`, for ARM.
    pub variant: Option<String>,
    /// Such as `linux`.
    pub os: Option<String>,
    pub author: Option<String>,
    pub comment: Option<String>,
    /// Who runs its processes, unless a container says otherwise.
    pub user: Option<String>,
    pub working_directory: Option<String>,
    /// Such as `80/tcp`.
    pub exposed_ports: Vec<String>,
    /// Where it expects volumes.
    pub volumes: Vec<String>,
    pub stop_signal: Option<String>,
    pub layers: u32,
}

impl From<ImageDetail> for ImageDetailView {
    fn from(value: ImageDetail) -> Self {
        Self {
            image: value.image.into(),
            architecture: value.architecture,
            variant: value.variant,
            os: value.os,
            author: value.author,
            comment: value.comment,
            user: value.user,
            working_directory: value.working_directory,
            exposed_ports: value.exposed_ports,
            volumes: value.volumes,
            stop_signal: value.stop_signal,
            layers: value.layers,
        }
    }
}

/// What removing a reference to an image did.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ImageRemovalView {
    /// The image's ID before it was removed.
    pub id: String,
    /// The references removed.
    pub untagged: Vec<String>,
    /// The images and layers deleted; empty when other references remain.
    pub deleted: Vec<String>,
}

impl From<ImageRemoval> for ImageRemovalView {
    fn from(value: ImageRemoval) -> Self {
        Self {
            id: value.id,
            untagged: value.untagged,
            deleted: value.deleted,
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ImagesQuery {
    /// Matched against tags and IDs, ignoring case.
    search: Option<String>,
}

/// An enabled engine's images, with the containers made from each.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/images",
    params(("engine" = String, Path, description = "docker or podman"), ImagesQuery, QueryHeaders),
    responses((status = 200, body = ImageListView)), tag = "containers")]
pub(crate) async fn list_images<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    Query(query): Query<ImagesQuery>,
    headers: HeaderMap,
) -> Result<Json<ImageListView>, ApiError> {
    let images = state
        .images
        .images(
            request_scope(&headers)?,
            engine(name)?,
            query.search.unwrap_or_default(),
        )
        .await?;
    Ok(Json(images.into()))
}

/// An image's configuration, without its environment or command line.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/images/{image}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("image" = String, Path, description = "Its ID, a prefix of its ID or a reference, with slashes percent-encoded"),
        QueryHeaders,
    ),
    responses((status = 200, body = ImageDetailView)), tag = "containers")]
pub(crate) async fn inspect_image<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ImageDetailView>, ApiError> {
    let detail = state
        .images
        .inspect_image(request_scope(&headers)?, engine(name)?, image(reference)?)
        .await?;
    Ok(Json(detail.into()))
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RemoveImageQuery {
    /// Removes an image stopped containers use, and every reference to an
    /// image named by its ID. An image a running container uses is refused
    /// anyway.
    force: Option<bool>,
}

/// Removes a reference to an image, and the image once nothing names it.
/// An image the panel's own installation uses is never removed, and the
/// audit trail records each removal, refused or not.
#[utoipa::path(delete, path = "/api/v1/container-engines/{engine}/images/{image}",
    params(
        ("engine" = String, Path, description = "docker or podman"),
        ("image" = String, Path, description = "Its ID, a prefix of its ID or a reference, with slashes percent-encoded"),
        RemoveImageQuery,
        MutationHeaders,
    ),
    responses((status = 200, body = ImageRemovalView)), tag = "containers")]
pub(crate) async fn remove_image<U>(
    State(state): State<ApiState<U>>,
    Path((name, reference)): Path<(String, String)>,
    Query(query): Query<RemoveImageQuery>,
    headers: HeaderMap,
) -> Result<Json<ImageRemovalView>, ApiError> {
    let removal = state
        .images
        .remove_image(
            command_context(&headers)?,
            engine(name)?,
            image(reference)?,
            query.force.unwrap_or(false),
        )
        .await?;
    Ok(Json(removal.into()))
}
