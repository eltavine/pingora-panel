//! What the container engines the host agent reaches keep besides
//! containers and images (ADR 0031): their networks and volumes.

use crate::{
    containers::engine,
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    EngineDiskUsage, EngineDiskUse, EngineNetwork, EngineNetworkList, EngineVolume,
    EngineVolumeList,
};
use serde::Serialize;
use std::{collections::BTreeMap, time::SystemTime};
use utoipa::ToSchema;

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
