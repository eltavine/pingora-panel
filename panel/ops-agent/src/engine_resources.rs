//! What the engines the agent reaches keep besides containers and images
//! (ADR 0031): their networks and volumes, each with the containers that
//! use it.

use crate::{
    containers::{answer, failure, time, Engines, ACTION_TIMEOUT, COMPOSE_PROJECT},
    engine_disk, engine_prune,
};
use bollard::{
    models::{Network, Volume},
    query_parameters::{ListContainersOptionsBuilder, ListNetworksOptions, ListVolumesOptions},
    Docker,
};
use panel_contracts::ops::v1::{self as wire, engine_resources_server::EngineResources};
use panel_errors::PanelError;
use std::{collections::HashMap, sync::Arc, time::SystemTime};
use tonic::{Request, Response, Status};

/// How many containers, running or not, use each network and each volume,
/// by name.
#[derive(Default)]
struct Uses {
    networks: HashMap<String, u32>,
    volumes: HashMap<String, u32>,
}

async fn uses(client: &Docker) -> Result<Uses, PanelError> {
    let options = ListContainersOptionsBuilder::default().all(true).build();
    let mut uses = Uses::default();
    for container in client
        .list_containers(Some(options))
        .await
        .map_err(|error| failure(&error))?
    {
        let networks = container
            .network_settings
            .and_then(|settings| settings.networks)
            .unwrap_or_default();
        for name in networks.into_keys() {
            *uses.networks.entry(name).or_insert(0) += 1;
        }
        for mount in container.mounts.unwrap_or_default() {
            if let (Some("volume"), Some(name)) = (mount.typ.as_deref(), mount.name) {
                *uses.volumes.entry(name).or_insert(0) += 1;
            }
        }
    }
    Ok(uses)
}

fn network(value: Network, uses: &Uses) -> wire::EngineNetwork {
    let name = value.name.unwrap_or_default();
    let labels = value.labels.unwrap_or_default();
    wire::EngineNetwork {
        id: value.id.unwrap_or_default(),
        containers: uses.networks.get(&name).copied().unwrap_or(0),
        name,
        driver: value.driver.unwrap_or_default(),
        scope: value.scope.unwrap_or_default(),
        created: time(value.created.as_deref()).map(Into::into),
        internal: value.internal.unwrap_or(false),
        ipv6: value
            .options
            .as_ref()
            .and_then(|options| options.get("com.docker.network.enable_ipv6"))
            .is_some_and(|enabled| enabled == "true"),
        subnets: value
            .ipam
            .and_then(|ipam| ipam.config)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|config| {
                Some(wire::EngineSubnet {
                    subnet: config.subnet?,
                    gateway: config.gateway.unwrap_or_default(),
                })
            })
            .collect(),
        compose_project: labels.get(COMPOSE_PROJECT).cloned().unwrap_or_default(),
        labels: labels.into_iter().collect(),
    }
}

fn volume(value: Volume, uses: &Uses) -> wire::EngineVolume {
    wire::EngineVolume {
        containers: uses.volumes.get(&value.name).copied().unwrap_or(0),
        compose_project: value
            .labels
            .get(COMPOSE_PROJECT)
            .cloned()
            .unwrap_or_default(),
        name: value.name,
        driver: value.driver,
        mountpoint: value.mountpoint,
        created: time(value.created_at.as_deref()).map(Into::into),
        scope: value
            .scope
            .map(|scope| scope.to_string())
            .unwrap_or_default(),
        labels: value.labels.into_iter().collect(),
    }
}

/// The engines' networks and volumes to panel-api.
pub(crate) struct ResourceService {
    engines: Arc<Engines>,
    /// The Compose project of the panel's own installation.
    installation: String,
}

impl ResourceService {
    pub(crate) fn new(engines: Arc<Engines>, installation: String) -> Self {
        Self {
            engines,
            installation,
        }
    }

    async fn networks(&self, engine: &str) -> Result<Vec<wire::EngineNetwork>, PanelError> {
        let client = self.engines.enabled(engine)?;
        let networks = client
            .list_networks(None::<ListNetworksOptions>)
            .await
            .map_err(|error| failure(&error))?;
        let uses = uses(&client).await?;
        let mut networks: Vec<_> = networks
            .into_iter()
            .map(|value| network(value, &uses))
            .collect();
        networks.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(networks)
    }

    async fn volumes(&self, engine: &str) -> Result<Vec<wire::EngineVolume>, PanelError> {
        let client = self.engines.enabled(engine)?;
        let volumes = client
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(|error| failure(&error))?
            .volumes
            .unwrap_or_default();
        let uses = uses(&client).await?;
        let mut volumes: Vec<_> = volumes
            .into_iter()
            .map(|value| volume(value, &uses))
            .collect();
        volumes.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(volumes)
    }
}

#[tonic::async_trait]
impl EngineResources for ResourceService {
    async fn prune_preview(
        &self,
        request: Request<wire::EngineResourcesPrunePreviewRequest>,
    ) -> Result<Response<wire::EngineResourcesPrunePreviewResponse>, Status> {
        let request = request.into_inner();
        let read = async {
            let socket = self.engines.enabled_socket(&request.engine)?;
            let client = self.engines.enabled(&request.engine)?;
            let choices = request.choices.unwrap_or_default();
            engine_prune::preview(&client, socket, &self.installation, choices).await
        };
        let result = tokio::time::timeout(engine_prune::PREVIEW_TIMEOUT, read)
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not answer in time",
                ))
            });
        let (items, error) = answer(result);
        Ok(Response::new(wire::EngineResourcesPrunePreviewResponse {
            observed_at: items.as_ref().map(|_| SystemTime::now().into()),
            reclaimable_bytes: items.iter().flatten().map(|item| item.size_bytes).sum(),
            items: items.unwrap_or_default(),
            error,
        }))
    }

    async fn prune(
        &self,
        request: Request<wire::EngineResourcesPruneRequest>,
    ) -> Result<Response<wire::EngineResourcesPruneResponse>, Status> {
        let request = request.into_inner();
        let deadline = std::time::Instant::now() + engine_prune::PRUNE_BUDGET;
        let pruned = async {
            let socket = self.engines.enabled_socket(&request.engine)?;
            let client = self
                .engines
                .enabled(&request.engine)?
                .with_timeout(ACTION_TIMEOUT);
            let choices = request.choices.unwrap_or_default();
            engine_prune::prune(
                &client,
                socket,
                &self.installation,
                choices,
                request.items,
                deadline,
            )
            .await
        };
        let result = tokio::time::timeout(engine_prune::PRUNE_TIMEOUT, pruned)
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not finish in time",
                ))
            });
        Ok(Response::new(match result {
            Ok((outcomes, reclaimed_bytes)) => {
                tracing::info!(
                    event = "engine_pruned",
                    engine = %request.engine,
                    removed = outcomes.iter().filter(|outcome| outcome.error.is_none()).count(),
                    kept = outcomes.iter().filter(|outcome| outcome.error.is_some()).count(),
                    reclaimed_bytes,
                );
                wire::EngineResourcesPruneResponse {
                    outcomes,
                    reclaimed_bytes,
                    error: None,
                }
            }
            Err(error) => {
                tracing::warn!(
                    event = "engine_prune_refused",
                    engine = %request.engine,
                    error_code = %error.code,
                );
                wire::EngineResourcesPruneResponse {
                    error: Some((&error).into()),
                    ..wire::EngineResourcesPruneResponse::default()
                }
            }
        }))
    }

    async fn disk_usage(
        &self,
        request: Request<wire::EngineResourcesDiskUsageRequest>,
    ) -> Result<Response<wire::EngineResourcesDiskUsageResponse>, Status> {
        let read = async {
            let socket = self.engines.enabled_socket(&request.get_ref().engine)?;
            Ok(engine_disk::usage(&engine_disk::data_usage(socket).await?))
        };
        let result = tokio::time::timeout(engine_disk::USAGE_TIMEOUT, read)
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not add up its disk use in time",
                ))
            });
        Ok(Response::new(match result {
            Ok([images, containers, volumes, build_cache]) => {
                wire::EngineResourcesDiskUsageResponse {
                    observed_at: Some(SystemTime::now().into()),
                    images: Some(images),
                    containers: Some(containers),
                    volumes: Some(volumes),
                    build_cache: Some(build_cache),
                    error: None,
                }
            }
            Err(error) => wire::EngineResourcesDiskUsageResponse {
                error: Some((&error).into()),
                ..wire::EngineResourcesDiskUsageResponse::default()
            },
        }))
    }

    async fn list_networks(
        &self,
        request: Request<wire::EngineResourcesListNetworksRequest>,
    ) -> Result<Response<wire::EngineResourcesListNetworksResponse>, Status> {
        let (networks, error) = answer(self.networks(&request.get_ref().engine).await);
        Ok(Response::new(wire::EngineResourcesListNetworksResponse {
            observed_at: networks.as_ref().map(|_| SystemTime::now().into()),
            networks: networks.unwrap_or_default(),
            error,
        }))
    }

    async fn list_volumes(
        &self,
        request: Request<wire::EngineResourcesListVolumesRequest>,
    ) -> Result<Response<wire::EngineResourcesListVolumesResponse>, Status> {
        let (volumes, error) = answer(self.volumes(&request.get_ref().engine).await);
        Ok(Response::new(wire::EngineResourcesListVolumesResponse {
            observed_at: volumes.as_ref().map(|_| SystemTime::now().into()),
            volumes: volumes.unwrap_or_default(),
            error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_engine::{engine, engine_with, engines, Calls};
    use panel_contracts::ops::v1::EnginePruneKind;

    #[tokio::test]
    async fn networks_are_listed_with_their_subnets_and_containers() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let listed = service
            .list_networks(Request::new(wire::EngineResourcesListNetworksRequest {
                context: None,
                engine: "docker".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(listed.error.is_none(), "{:?}", listed.error);
        assert!(listed.observed_at.is_some());
        let names: Vec<_> = listed.networks.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["bridge", "host", "shop_default", "stale_net"]);
        let shop = &listed.networks[2];
        assert_eq!((shop.driver.as_str(), shop.containers), ("bridge", 1));
        assert_eq!(shop.compose_project, "shop");
        assert_eq!(shop.subnets[0].subnet, "172.18.0.0/16");
        assert_eq!(shop.subnets[0].gateway, "172.18.0.1");
        assert!(shop.ipv6 && !shop.internal);
        assert_eq!(listed.networks[1].containers, 0);
    }

    #[tokio::test]
    async fn disk_use_is_read_from_the_engine() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let usage = service
            .disk_usage(Request::new(wire::EngineResourcesDiskUsageRequest {
                context: None,
                engine: "docker".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(usage.error.is_none(), "{:?}", usage.error);
        assert!(usage.observed_at.is_some());
        let images = usage.images.unwrap();
        assert_eq!((images.total, images.size_bytes), (3, 150_000_000));
        assert_eq!(usage.volumes.unwrap().reclaimable_bytes, 1_024 + 2_048);

        let gone = ResourceService::new(
            engines(directory.path().join("missing.sock"), None),
            "pingora-panel".into(),
        );
        let unreachable = gone
            .disk_usage(Request::new(wire::EngineResourcesDiskUsageRequest {
                context: None,
                engine: "docker".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(unreachable.error.unwrap().code, "UNAVAILABLE");
    }

    #[tokio::test]
    async fn volumes_are_listed_with_the_containers_that_mount_them() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let listed = service
            .list_volumes(Request::new(wire::EngineResourcesListVolumesRequest {
                context: None,
                engine: "docker".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(listed.error.is_none(), "{:?}", listed.error);
        let names: Vec<_> = listed.volumes.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["3f9a1c", "orphan", "shop_html"]);
        let html = &listed.volumes[2];
        assert_eq!(
            (html.containers, html.compose_project.as_str()),
            (1, "shop")
        );
        assert_eq!(html.mountpoint, "/var/lib/docker/volumes/shop_html/_data");
        assert_eq!(
            (html.driver.as_str(), html.scope.as_str()),
            ("local", "local")
        );
        assert_eq!(html.created.unwrap().seconds, 1_800_000_000);
        assert_eq!(listed.volumes[1].containers, 0);
    }

    fn preview_of(items: &[wire::EnginePruneItem]) -> Vec<(i32, &str, u64)> {
        items
            .iter()
            .map(|item| (item.kind, item.id.as_str(), item.size_bytes))
            .collect()
    }

    async fn previewed(
        service: &ResourceService,
        named_volumes: bool,
    ) -> wire::EngineResourcesPrunePreviewResponse {
        service
            .prune_preview(Request::new(wire::EngineResourcesPrunePreviewRequest {
                context: None,
                engine: "docker".into(),
                choices: Some(wire::EnginePruneChoices {
                    tagged_images: true,
                    named_volumes,
                }),
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn a_prune_preview_lists_only_what_nothing_uses() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(
            engines(engine(directory.path()).await, None),
            "pingora-panel".into(),
        );
        let preview = previewed(&service, false).await;
        assert!(preview.error.is_none(), "{:?}", preview.error);
        assert_eq!(
            preview_of(&preview.items),
            [
                (EnginePruneKind::Container as i32, "a1", 1_024),
                (EnginePruneKind::Image as i32, "sha256:cc", 50_000_000),
                (EnginePruneKind::Volume as i32, "3f9a1c", 2_048),
                (EnginePruneKind::Network as i32, "n4", 0),
                (EnginePruneKind::BuildCache as i32, "c1", 700),
            ],
            "running, used and built-in things stay, and so do named volumes"
        );
        assert_eq!(preview.reclaimable_bytes, 1_024 + 50_000_000 + 2_048 + 700);
        assert_eq!(preview.items[0].name, "cache");
        assert_eq!(preview.items[4].name, "mount / from exec /bin/sh -c make");

        let named = previewed(&service, true).await;
        assert!(named.items.iter().any(|item| item.id == "orphan"));
    }

    #[tokio::test]
    async fn pruning_removes_only_what_a_preview_still_lists() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let service = ResourceService::new(
            engines(engine_with(directory.path(), calls.clone()).await, None),
            "pingora-panel".into(),
        );
        let mut items = previewed(&service, false).await.items;
        items.push(wire::EnginePruneItem {
            kind: EnginePruneKind::Volume.into(),
            id: "orphan".into(),
            name: "orphan".into(),
            size_bytes: 1_024,
        });
        items.push(wire::EnginePruneItem {
            kind: EnginePruneKind::Container.into(),
            id: "b2".into(),
            name: "shop-web-1".into(),
            size_bytes: 0,
        });
        let pruned = service
            .prune(Request::new(wire::EngineResourcesPruneRequest {
                context: None,
                engine: "docker".into(),
                choices: Some(wire::EnginePruneChoices {
                    tagged_images: true,
                    named_volumes: false,
                }),
                items,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(pruned.error.is_none(), "{:?}", pruned.error);
        let kept: Vec<_> = pruned
            .outcomes
            .iter()
            .filter_map(|outcome| {
                Some((
                    outcome.item.as_ref()?.id.as_str(),
                    outcome.error.as_ref()?.code.as_str(),
                ))
            })
            .collect();
        assert_eq!(
            kept,
            [
                ("orphan", "PRECONDITION_FAILED"),
                ("b2", "PRECONDITION_FAILED")
            ],
            "a named volume and a running container were never in the preview"
        );
        assert_eq!(pruned.reclaimed_bytes, 1_024 + 50_000_000 + 2_048 + 700);
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "remove a1 force=false volumes=false",
                "remove-image sha256:cc force=true",
                "remove-volume 3f9a1c",
                "remove-network n4",
                "prune-build c1"
            ]
        );
    }
}
