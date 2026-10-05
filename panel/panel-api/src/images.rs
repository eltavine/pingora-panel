//! The images on the container engines the host agent reaches (ADR 0031).

use crate::{
    containers::engine,
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    tail::LogTailError,
    ApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{Stream, StreamExt};
use panel_application::{
    Image, ImageDetail, ImageLayerProgress, ImageLayerState, ImageList, ImagePullEvent,
    ImagePullRequest, ImageRemoval, RegistryCredentials,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, convert::Infallible, time::SystemTime};
use tokio::sync::mpsc::{self, error::TrySendError};
use utoipa::{IntoParams, ToSchema};
use zeroize::Zeroizing;

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

/// For a registry that wants a sign-in: used for this pull only, never kept
/// or recorded.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryCredentialsBody {
    #[schema(max_length = 256)]
    pub username: String,
    /// A password or an access token.
    #[schema(value_type = String, format = Password, max_length = 8192)]
    pub password: Zeroizing<String>,
}

/// An image to pull.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImagePullBody {
    /// Such as `nginx:1.27` or `ghcr.io/example/app@sha256:…`; a name alone,
    /// such as `nginx`, is `nginx:latest`.
    #[schema(min_length = 1, max_length = 512)]
    pub reference: String,
    /// Such as `linux/arm64`; the engine's own when absent.
    #[schema(max_length = 64)]
    pub platform: Option<String>,
    pub credentials: Option<RegistryCredentialsBody>,
}

/// Where a layer of an image being pulled is.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImageLayerStateName {
    /// Behind other layers, or about to be retried.
    Waiting,
    Downloading,
    /// Downloaded and checked against its digest.
    Downloaded,
    Extracting,
    Complete,
    /// The engine had it already.
    Exists,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ImageLayerView {
    /// Such as `a2abf6c4d29d`.
    pub id: String,
    pub state: ImageLayerStateName,
    /// How far downloading or extracting it got, of how much; 0 while
    /// unknown.
    pub current_bytes: u64,
    pub total_bytes: u64,
}

impl From<ImageLayerProgress> for ImageLayerView {
    fn from(value: ImageLayerProgress) -> Self {
        Self {
            state: match value.state {
                ImageLayerState::Downloading => ImageLayerStateName::Downloading,
                ImageLayerState::Downloaded => ImageLayerStateName::Downloaded,
                ImageLayerState::Extracting => ImageLayerStateName::Extracting,
                ImageLayerState::Complete => ImageLayerStateName::Complete,
                ImageLayerState::Exists => ImageLayerStateName::Exists,
                _ => ImageLayerStateName::Waiting,
            },
            id: value.id,
            current_bytes: value.current_bytes,
            total_bytes: value.total_bytes,
        }
    }
}

/// What a pull says as it goes, one per event: how far its layers got, then
/// what it pulled or why it failed, which is the last.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImagePullMessage {
    Progress {
        /// In the order the engine first named them.
        layers: Vec<ImageLayerView>,
    },
    Pulled {
        image: ImageView,
        /// Such as `sha256:…`: what the registry served.
        digest: Option<String>,
        /// Whether the engine downloaded a newer image than it had.
        updated: bool,
    },
    Failed {
        error: LogTailError,
    },
}

impl From<Result<ImagePullEvent, PanelError>> for ImagePullMessage {
    fn from(value: Result<ImagePullEvent, PanelError>) -> Self {
        match value {
            Ok(ImagePullEvent::Progress(layers)) => Self::Progress {
                layers: layers.into_iter().map(Into::into).collect(),
            },
            Ok(ImagePullEvent::Pulled(pulled)) => Self::Pulled {
                image: pulled.image.into(),
                digest: pulled.digest,
                updated: pulled.updated,
            },
            Err(error) => Self::Failed {
                error: LogTailError::from(&error),
            },
        }
    }
}

/// How many messages wait for a slow client; progress beyond them is
/// dropped, since each says all there is.
const PULL_BACKLOG: usize = 8;

/// Pulls an image from its registry as Server-Sent Events, each a JSON
/// message: how far its layers got, at most four times a second, then what
/// it pulled or why it failed. A refusal before anything is pulled answers
/// as any other. A pull goes on to its end when its client leaves, and the
/// audit trail records each, refused or not.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/image-pulls",
    params(("engine" = String, Path, description = "docker or podman"), MutationHeaders),
    request_body = ImagePullBody,
    responses((status = 200, content_type = "text/event-stream", body = ImagePullMessage)),
    tag = "containers")]
pub(crate) async fn pull_image<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ImagePullBody>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let platform = body
        .platform
        .map(|platform| platform.trim().to_owned())
        .filter(|platform| !platform.is_empty());
    if platform
        .as_ref()
        .is_some_and(|platform| platform.len() > 64)
        || body.credentials.as_ref().is_some_and(|credentials| {
            credentials.username.len() > 256 || credentials.password.len() > 8192
        })
    {
        return Err(ApiError::new(PanelError::invalid_argument(
            "the platform or the credentials are too long",
        )));
    }
    let request = ImagePullRequest {
        reference: image(body.reference.trim().to_owned())?,
        platform,
        credentials: body.credentials.map(|credentials| RegistryCredentials {
            username: credentials.username,
            password: credentials.password,
        }),
    };
    let mut pull = state
        .images
        .pull_image(command_context(&headers)?, engine(name)?, request)
        .await?;
    let first = match pull.next().await {
        Some(Ok(event)) => event,
        Some(Err(error)) => return Err(ApiError::new(error)),
        None => {
            return Err(ApiError::new(PanelError::unavailable(
                "the pull ended before it began",
            )))
        }
    };
    let (sender, mut receiver) = mpsc::channel(PULL_BACKLOG);
    tokio::spawn(async move {
        let mut listening = sender.send(Ok(first)).await.is_ok();
        while let Some(event) = pull.next().await {
            if !listening {
                continue;
            }
            listening = match event {
                Ok(ImagePullEvent::Progress(_)) => {
                    !matches!(sender.try_send(event), Err(TrySendError::Closed(_)))
                }
                event => sender.send(event).await.is_ok(),
            };
        }
    });
    let events =
        futures_util::stream::poll_fn(move |context| receiver.poll_recv(context)).map(|event| {
            let message = ImagePullMessage::from(event);
            Ok(Event::default()
                .json_data(&message)
                .unwrap_or_else(|_| Event::default().comment("unserializable")))
        });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}
