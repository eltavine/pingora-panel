//! What the engines the agent reaches keep besides containers and images
//! (ADR 0031): their networks and volumes, each with the containers that
//! use it.

use crate::containers::{answer, failure, time, Engines, COMPOSE_PROJECT};
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
}

impl ResourceService {
    pub(crate) fn new(engines: Arc<Engines>) -> Self {
        Self { engines }
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
    use crate::fake_engine::{engine, engines};

    #[tokio::test]
    async fn networks_are_listed_with_their_subnets_and_containers() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(engines(engine(directory.path()).await, None));
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
        assert_eq!(names, ["bridge", "host", "shop_default"]);
        let shop = &listed.networks[2];
        assert_eq!((shop.driver.as_str(), shop.containers), ("bridge", 1));
        assert_eq!(shop.compose_project, "shop");
        assert_eq!(shop.subnets[0].subnet, "172.18.0.0/16");
        assert_eq!(shop.subnets[0].gateway, "172.18.0.1");
        assert!(shop.ipv6 && !shop.internal);
        assert_eq!(listed.networks[1].containers, 0);
    }

    #[tokio::test]
    async fn volumes_are_listed_with_the_containers_that_mount_them() {
        let directory = tempfile::tempdir().unwrap();
        let service = ResourceService::new(engines(engine(directory.path()).await, None));
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
        assert_eq!(names, ["orphan", "shop_html"]);
        let html = &listed.volumes[1];
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
        assert_eq!(listed.volumes[0].containers, 0);
    }
}
