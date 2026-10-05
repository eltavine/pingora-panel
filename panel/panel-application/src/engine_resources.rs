//! What the container engines `ops-agent` reaches keep besides containers
//! and images (ADR 0031): their networks and volumes.

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

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

/// What pruning removes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PruneKind {
    Container,
    Image,
    Volume,
    Network,
    BuildCache,
}

impl PruneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Image => "image",
            Self::Volume => "volume",
            Self::Network => "network",
            Self::BuildCache => "build_cache",
        }
    }
}

/// Something pruning would remove.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PruneItem {
    pub kind: PruneKind,
    /// Its ID, or a volume's name.
    pub id: String,
    /// How people know it: a container's, volume's or network's name, an
    /// image's tag, what a build cache record holds.
    pub name: String,
    /// What removing it frees, as far as the engine says.
    pub size_bytes: u64,
}

/// What pruning looks at besides containers, images nothing names,
/// anonymous volumes, networks and build cache.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PruneChoices {
    /// Images a tag still names but no container uses.
    pub tagged_images: bool,
    /// Volumes a name was given, which usually hold data someone meant to
    /// keep.
    pub named_volumes: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrunePreview {
    pub observed_at: Option<SystemTime>,
    /// By kind, then name.
    pub items: Vec<PruneItem>,
    /// What removing them all would free.
    pub reclaimable_bytes: u64,
}

/// What became of one item a prune was asked to remove.
#[derive(Clone, Debug)]
pub struct PruneOutcome {
    pub item: PruneItem,
    /// Why it stayed; `None` once it is gone.
    pub refusal: Option<PanelError>,
}

#[derive(Clone, Debug)]
pub struct PruneReport {
    /// In the order asked.
    pub outcomes: Vec<PruneOutcome>,
    /// What the items that went freed.
    pub reclaimed_bytes: u64,
}

#[async_trait]
pub trait EngineResourcesPort: Send + Sync {
    async fn networks(&self, scope: RequestScope, engine: String) -> Result<EngineNetworkList>;

    async fn volumes(&self, scope: RequestScope, engine: String) -> Result<EngineVolumeList>;

    async fn disk_usage(&self, scope: RequestScope, engine: String) -> Result<EngineDiskUsage>;

    /// What pruning an engine with `choices` would remove; never what the
    /// panel's own installation made.
    async fn prune_preview(
        &self,
        scope: RequestScope,
        engine: String,
        choices: PruneChoices,
    ) -> Result<PrunePreview>;

    /// Removes the items of a preview with `choices` that a fresh preview
    /// still lists.
    async fn prune(
        &self,
        context: CommandContext,
        engine: String,
        choices: PruneChoices,
        items: Vec<PruneItem>,
    ) -> Result<PruneReport>;
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

    async fn prune_preview(
        &self,
        _: RequestScope,
        _: String,
        _: PruneChoices,
    ) -> Result<PrunePreview> {
        Err(Self::refusal())
    }

    async fn prune(
        &self,
        _: CommandContext,
        _: String,
        _: PruneChoices,
        _: Vec<PruneItem>,
    ) -> Result<PruneReport> {
        Err(Self::refusal())
    }
}

/// An engine resources port that records each prune, refused or not.
pub struct RecordedEngineResources {
    inner: Arc<dyn EngineResourcesPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedEngineResources {
    pub fn new(inner: Arc<dyn EngineResourcesPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl EngineResourcesPort for RecordedEngineResources {
    async fn networks(&self, scope: RequestScope, engine: String) -> Result<EngineNetworkList> {
        self.inner.networks(scope, engine).await
    }

    async fn volumes(&self, scope: RequestScope, engine: String) -> Result<EngineVolumeList> {
        self.inner.volumes(scope, engine).await
    }

    async fn disk_usage(&self, scope: RequestScope, engine: String) -> Result<EngineDiskUsage> {
        self.inner.disk_usage(scope, engine).await
    }

    async fn prune_preview(
        &self,
        scope: RequestScope,
        engine: String,
        choices: PruneChoices,
    ) -> Result<PrunePreview> {
        self.inner.prune_preview(scope, engine, choices).await
    }

    async fn prune(
        &self,
        context: CommandContext,
        engine: String,
        choices: PruneChoices,
        items: Vec<PruneItem>,
    ) -> Result<PruneReport> {
        let result = self
            .inner
            .prune(context.clone(), engine.clone(), choices, items)
            .await;
        let operation = Operation::EnginePrune {
            engine: &engine,
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use std::sync::Mutex;

    /// Removes the stopped container and refuses the rest of a prune.
    struct Engine;

    #[async_trait]
    impl EngineResourcesPort for Engine {
        async fn networks(&self, _: RequestScope, _: String) -> Result<EngineNetworkList> {
            Err(NoEngineResources::refusal())
        }

        async fn volumes(&self, _: RequestScope, _: String) -> Result<EngineVolumeList> {
            Err(NoEngineResources::refusal())
        }

        async fn disk_usage(&self, _: RequestScope, _: String) -> Result<EngineDiskUsage> {
            Err(NoEngineResources::refusal())
        }

        async fn prune_preview(
            &self,
            _: RequestScope,
            _: String,
            _: PruneChoices,
        ) -> Result<PrunePreview> {
            Err(NoEngineResources::refusal())
        }

        async fn prune(
            &self,
            _: CommandContext,
            engine: String,
            _: PruneChoices,
            items: Vec<PruneItem>,
        ) -> Result<PruneReport> {
            if engine != "docker" {
                return Err(PanelError::not_found(format!("no engine named {engine}")));
            }
            Ok(PruneReport {
                reclaimed_bytes: 1_024,
                outcomes: items
                    .into_iter()
                    .map(|item| PruneOutcome {
                        refusal: (item.kind != PruneKind::Container)
                            .then(|| PanelError::conflict("in use")),
                        item,
                    })
                    .collect(),
            })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::EnginePrune { engine, result } = operation {
                let outcome = match result {
                    Ok(report) => format!(
                        "{} removed, {} bytes",
                        report
                            .outcomes
                            .iter()
                            .filter(|o| o.refusal.is_none())
                            .count(),
                        report.reclaimed_bytes
                    ),
                    Err(error) => error.code.as_str().to_owned(),
                };
                self.0.lock().unwrap().push(format!("{engine}: {outcome}"));
            }
        }
    }

    fn item(kind: PruneKind, name: &str) -> PruneItem {
        PruneItem {
            kind,
            id: name.into(),
            name: name.into(),
            size_bytes: 1_024,
        }
    }

    #[tokio::test]
    async fn every_prune_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let resources = RecordedEngineResources::new(Arc::new(Engine), recorder.clone());
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap();
        let items = vec![
            item(PruneKind::Container, "cache"),
            item(PruneKind::Volume, "orphan"),
        ];
        resources
            .prune(
                context.clone(),
                "docker".into(),
                PruneChoices::default(),
                items,
            )
            .await
            .unwrap();
        resources
            .prune(
                context,
                "podman".into(),
                PruneChoices::default(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            ["docker: 1 removed, 1024 bytes", "podman: NOT_FOUND"]
        );
    }
}
