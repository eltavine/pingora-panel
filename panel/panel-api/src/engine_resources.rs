//! What the container engines the host agent reaches keep besides
//! containers and images (ADR 0031): their networks and volumes.

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
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    EngineDiskUsage, EngineDiskUse, EngineNetwork, EngineNetworkList, EngineVolume,
    EngineVolumeList, PruneChoices, PruneItem, PruneKind, PrunePreview, PruneReport,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineSubnetView {
    /// Such as `172.18.0.0/16`.
    pub subnet: String,
    pub gateway: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineNetworkView {
    pub id: String,
    pub name: String,
    /// Such as `bridge`, `host`, `overlay` or `macvlan`.
    pub driver: String,
    /// `local`, `global` or `swarm`.
    pub scope: String,
    /// RFC 3339.
    pub created: Option<String>,
    /// Whether containers on it are cut off from outside networks.
    pub internal: bool,
    pub ipv6: bool,
    pub subnets: Vec<EngineSubnetView>,
    /// The containers, running or not, attached to it.
    pub containers: u32,
    /// The Compose project that created it, if one did.
    pub compose_project: Option<String>,
    pub labels: BTreeMap<String, String>,
}

impl From<EngineNetwork> for EngineNetworkView {
    fn from(value: EngineNetwork) -> Self {
        Self {
            id: value.id,
            name: value.name,
            driver: value.driver,
            scope: value.scope,
            created: value.created.map(rfc3339),
            internal: value.internal,
            ipv6: value.ipv6,
            subnets: value
                .subnets
                .into_iter()
                .map(|subnet| EngineSubnetView {
                    subnet: subnet.subnet,
                    gateway: subnet.gateway,
                })
                .collect(),
            containers: value.containers,
            compose_project: value.compose_project,
            labels: value.labels,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineNetworkListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// By name.
    pub networks: Vec<EngineNetworkView>,
}

impl From<EngineNetworkList> for EngineNetworkListView {
    fn from(value: EngineNetworkList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            networks: value.networks.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineVolumeView {
    pub name: String,
    /// Such as `local`.
    pub driver: String,
    /// Where its data lives on the host.
    pub mountpoint: String,
    /// RFC 3339.
    pub created: Option<String>,
    /// `local` or `global`.
    pub scope: String,
    /// The containers, running or not, that mount it.
    pub containers: u32,
    /// The Compose project that created it, if one did.
    pub compose_project: Option<String>,
    pub labels: BTreeMap<String, String>,
}

impl From<EngineVolume> for EngineVolumeView {
    fn from(value: EngineVolume) -> Self {
        Self {
            name: value.name,
            driver: value.driver,
            mountpoint: value.mountpoint,
            created: value.created.map(rfc3339),
            scope: value.scope,
            containers: value.containers,
            compose_project: value.compose_project,
            labels: value.labels,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineVolumeListView {
    /// When the agent read them, RFC 3339.
    pub observed_at: Option<String>,
    /// By name.
    pub volumes: Vec<EngineVolumeView>,
}

impl From<EngineVolumeList> for EngineVolumeListView {
    fn from(value: EngineVolumeList) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            volumes: value.volumes.into_iter().map(Into::into).collect(),
        }
    }
}

/// An enabled engine's networks, with their subnets and the containers
/// attached to each.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/networks",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = EngineNetworkListView)), tag = "containers")]
pub(crate) async fn list_networks<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<EngineNetworkListView>, ApiError> {
    let networks = state
        .resources
        .networks(request_scope(&headers)?, engine(name)?)
        .await?;
    Ok(Json(networks.into()))
}

/// An enabled engine's volumes, with where their data lives and the
/// containers that mount each.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/volumes",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = EngineVolumeListView)), tag = "containers")]
pub(crate) async fn list_volumes<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<EngineVolumeListView>, ApiError> {
    let volumes = state
        .resources
        .volumes(request_scope(&headers)?, engine(name)?)
        .await?;
    Ok(Json(volumes.into()))
}

/// One kind of thing an engine keeps on disk.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct EngineDiskUseView {
    pub total: u32,
    /// In use: running containers, images and volumes a container uses, and
    /// build cache in use.
    pub active: u32,
    pub size_bytes: u64,
    /// What removing what is not in use would free.
    pub reclaimable_bytes: u64,
}

impl From<EngineDiskUse> for EngineDiskUseView {
    fn from(value: EngineDiskUse) -> Self {
        Self {
            total: value.total,
            active: value.active,
            size_bytes: value.size_bytes,
            reclaimable_bytes: value.reclaimable_bytes,
        }
    }
}

/// The disk an engine takes, as `docker system df` reports it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineDiskUsageView {
    /// When the agent read it, RFC 3339.
    pub observed_at: Option<String>,
    pub images: EngineDiskUseView,
    pub containers: EngineDiskUseView,
    /// Local volumes.
    pub volumes: EngineDiskUseView,
    pub build_cache: EngineDiskUseView,
}

impl From<EngineDiskUsage> for EngineDiskUsageView {
    fn from(value: EngineDiskUsage) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            images: value.images.into(),
            containers: value.containers.into(),
            volumes: value.volumes.into(),
            build_cache: value.build_cache.into(),
        }
    }
}

/// How much disk an enabled engine's images, containers, volumes and build
/// cache take, and how much removing what is not in use would free. The
/// engine sizes every volume, so this can take a while.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/disk-usage",
    params(("engine" = String, Path, description = "docker or podman"), QueryHeaders),
    responses((status = 200, body = EngineDiskUsageView)), tag = "containers")]
pub(crate) async fn disk_usage<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<EngineDiskUsageView>, ApiError> {
    let usage = state
        .resources
        .disk_usage(request_scope(&headers)?, engine(name)?)
        .await?;
    Ok(Json(usage.into()))
}

/// The most items one prune removes.
const MOST_PRUNED: usize = 1_000;

/// What pruning removes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PruneKindName {
    Container,
    Image,
    Volume,
    Network,
    BuildCache,
}

impl From<PruneKind> for PruneKindName {
    fn from(value: PruneKind) -> Self {
        match value {
            PruneKind::Container => Self::Container,
            PruneKind::Image => Self::Image,
            PruneKind::Volume => Self::Volume,
            PruneKind::Network => Self::Network,
            PruneKind::BuildCache => Self::BuildCache,
        }
    }
}

impl From<PruneKindName> for PruneKind {
    fn from(value: PruneKindName) -> Self {
        match value {
            PruneKindName::Container => Self::Container,
            PruneKindName::Image => Self::Image,
            PruneKindName::Volume => Self::Volume,
            PruneKindName::Network => Self::Network,
            PruneKindName::BuildCache => Self::BuildCache,
        }
    }
}

/// Something pruning would remove.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PruneItemView {
    pub kind: PruneKindName,
    /// Its ID, or a volume's name.
    pub id: String,
    /// How people know it: a container's, volume's or network's name, an
    /// image's tag, what a build cache record holds.
    pub name: String,
    /// What removing it frees, as far as the engine says.
    pub size_bytes: u64,
}

impl From<PruneItem> for PruneItemView {
    fn from(value: PruneItem) -> Self {
        Self {
            kind: value.kind.into(),
            id: value.id,
            name: value.name,
            size_bytes: value.size_bytes,
        }
    }
}

/// An item as a client sends it back: its ID is an engine's, by the
/// characters IDs and volume names use.
fn prune_item(value: PruneItemView) -> Result<PruneItem, ApiError> {
    let valid = !value.id.is_empty()
        && value.id.len() <= 255
        && value
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
    if !valid || value.name.len() > 512 {
        return Err(ApiError::new(PanelError::invalid_argument(format!(
            "`{}` is not an item pruning removes",
            value.id
        ))));
    }
    Ok(PruneItem {
        kind: value.kind.into(),
        id: value.id,
        name: value.name,
        size_bytes: value.size_bytes,
    })
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PruneChoicesQuery {
    /// Images a tag still names but no container uses, too.
    tagged_images: Option<bool>,
    /// Volumes a name was given too; they usually hold data someone meant
    /// to keep.
    named_volumes: Option<bool>,
}

/// What pruning would remove, by kind then name.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PrunePreviewView {
    /// When the agent looked, RFC 3339.
    pub observed_at: Option<String>,
    pub items: Vec<PruneItemView>,
    /// What removing them all would free.
    pub reclaimable_bytes: u64,
}

impl From<PrunePreview> for PrunePreviewView {
    fn from(value: PrunePreview) -> Self {
        Self {
            observed_at: value.observed_at.map(rfc3339),
            items: value.items.into_iter().map(Into::into).collect(),
            reclaimable_bytes: value.reclaimable_bytes,
        }
    }
}

/// What pruning an enabled engine would remove: containers that are not
/// running, images no container uses, anonymous volumes nothing mounts,
/// networks no container is on and build cache not in use. What the
/// panel's own installation made, and the engine's own networks, are never
/// listed.
#[utoipa::path(get, path = "/api/v1/container-engines/{engine}/prune-preview",
    params(("engine" = String, Path, description = "docker or podman"), PruneChoicesQuery, QueryHeaders),
    responses((status = 200, body = PrunePreviewView)), tag = "containers")]
pub(crate) async fn prune_preview<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    Query(query): Query<PruneChoicesQuery>,
    headers: HeaderMap,
) -> Result<Json<PrunePreviewView>, ApiError> {
    let choices = PruneChoices {
        tagged_images: query.tagged_images.unwrap_or(false),
        named_volumes: query.named_volumes.unwrap_or(false),
    };
    let preview = state
        .resources
        .prune_preview(request_scope(&headers)?, engine(name)?, choices)
        .await?;
    Ok(Json(preview.into()))
}

/// The items of a preview to remove, with the choices it was made with.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PruneRequest {
    #[serde(default)]
    pub tagged_images: bool,
    #[serde(default)]
    pub named_volumes: bool,
    /// At most 1000.
    pub items: Vec<PruneItemView>,
}

/// What became of one item.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PruneOutcomeView {
    pub item: PruneItemView,
    /// Why it stayed; absent once it is gone.
    pub error: Option<LogTailError>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PruneReportView {
    /// In the order asked.
    pub outcomes: Vec<PruneOutcomeView>,
    /// What the items that went freed.
    pub reclaimed_bytes: u64,
}

impl From<PruneReport> for PruneReportView {
    fn from(value: PruneReport) -> Self {
        Self {
            outcomes: value
                .outcomes
                .into_iter()
                .map(|outcome| PruneOutcomeView {
                    error: outcome.refusal.as_ref().map(LogTailError::from),
                    item: outcome.item.into(),
                })
                .collect(),
            reclaimed_bytes: value.reclaimed_bytes,
        }
    }
}

/// Removes the items of a preview that a fresh preview with the same
/// choices still lists, one at a time, so what came into use meanwhile
/// stays. The audit trail records each prune, refused or not.
#[utoipa::path(post, path = "/api/v1/container-engines/{engine}/prune",
    params(("engine" = String, Path, description = "docker or podman"), MutationHeaders),
    request_body = PruneRequest,
    responses((status = 200, body = PruneReportView)), tag = "containers")]
pub(crate) async fn prune<U>(
    State(state): State<ApiState<U>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PruneRequest>,
) -> Result<Json<PruneReportView>, ApiError> {
    if request.items.len() > MOST_PRUNED {
        return Err(ApiError::new(PanelError::invalid_argument(format!(
            "prune at most {MOST_PRUNED} items at a time"
        ))));
    }
    let choices = PruneChoices {
        tagged_images: request.tagged_images,
        named_volumes: request.named_volumes,
    };
    let items = request
        .items
        .into_iter()
        .map(prune_item)
        .collect::<Result<Vec<_>, _>>()?;
    let report = state
        .resources
        .prune(command_context(&headers)?, engine(name)?, choices, items)
        .await?;
    Ok(Json(report.into()))
}
