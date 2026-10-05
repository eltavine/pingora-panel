//! What the container engines `ops-agent` reaches keep besides containers
//! and images (ADR 0031): their networks and volumes.

use crate::RequestScope;
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, time::SystemTime};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineSubnet {
    /// Such as `172.18.0.0/16`.
    pub subnet: String,
    pub gateway: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineNetwork {
    pub id: String,
    pub name: String,
    /// Such as `bridge`, `host`, `overlay` or `macvlan`.
    pub driver: String,
    /// `local`, `global` or `swarm`.
    pub scope: String,
    pub created: Option<SystemTime>,
    /// Whether containers on it are cut off from outside networks.
    pub internal: bool,
    pub ipv6: bool,
    pub subnets: Vec<EngineSubnet>,
    /// The containers, running or not, attached to it.
    pub containers: u32,
    /// The Compose project that created it, if one did.
    pub compose_project: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineNetworkList {
    pub observed_at: Option<SystemTime>,
    /// By name.
    pub networks: Vec<EngineNetwork>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineVolume {
    pub name: String,
    /// Such as `local`.
    pub driver: String,
    /// Where its data lives on the host.
    pub mountpoint: String,
    pub created: Option<SystemTime>,
    /// `local` or `global`.
    pub scope: String,
    /// The containers, running or not, that mount it.
    pub containers: u32,
    /// The Compose project that created it, if one did.
    pub compose_project: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineVolumeList {
    pub observed_at: Option<SystemTime>,
    /// By name.
    pub volumes: Vec<EngineVolume>,
}

/// One kind of thing an engine keeps on disk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineDiskUse {
    pub total: u32,
    /// In use: running containers, images and volumes a container uses, and
    /// build cache in use.
    pub active: u32,
    pub size_bytes: u64,
    /// What removing what is not in use would free.
    pub reclaimable_bytes: u64,
}

/// The disk an engine takes, as `docker system df` reports it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineDiskUsage {
    pub observed_at: Option<SystemTime>,
    pub images: EngineDiskUse,
    pub containers: EngineDiskUse,
    /// Local volumes.
    pub volumes: EngineDiskUse,
    pub build_cache: EngineDiskUse,
}

#[async_trait]
pub trait EngineResourcesPort: Send + Sync {
    async fn networks(&self, scope: RequestScope, engine: String) -> Result<EngineNetworkList>;

    async fn volumes(&self, scope: RequestScope, engine: String) -> Result<EngineVolumeList>;

    async fn disk_usage(&self, scope: RequestScope, engine: String) -> Result<EngineDiskUsage>;
}

/// The port of an installation whose agent manages no engine.
pub struct NoEngineResources;

impl NoEngineResources {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("no host agent manages container engines here")
    }
}

#[async_trait]
impl EngineResourcesPort for NoEngineResources {
    async fn networks(&self, _: RequestScope, _: String) -> Result<EngineNetworkList> {
        Err(Self::refusal())
    }

    async fn volumes(&self, _: RequestScope, _: String) -> Result<EngineVolumeList> {
        Err(Self::refusal())
    }

    async fn disk_usage(&self, _: RequestScope, _: String) -> Result<EngineDiskUsage> {
        Err(Self::refusal())
    }
}
