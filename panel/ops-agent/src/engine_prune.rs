//! Pruning an engine (ADR 0031): a preview lists each item pruning would
//! remove with what removing it frees, and pruning removes the requested
//! items a preview with the same choices still lists, one at a time, so the
//! engine refuses whatever came into use meanwhile. What the panel's own
//! installation made, and the engine's own networks, are never listed.

use crate::{
    containers::{failure, COMPOSE_PROJECT},
    engine_disk,
    images::named,
};
use bollard::{
    models::{ContainerSummary, ContainerSummaryStateEnum},
    query_parameters::{
        ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListNetworksOptions,
        ListVolumesOptions, PruneBuildOptionsBuilder, RemoveContainerOptionsBuilder,
        RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder,
    },
    Docker,
};
use panel_contracts::ops::v1::{self as wire, EnginePruneKind};
use panel_errors::PanelError;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

/// The most items one prune removes.
pub(crate) const MOST_ITEMS: usize = 1_000;
/// How long a preview may take; the engine sizes every volume.
pub(crate) const PREVIEW_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a prune keeps starting removals, well within the deadline of
/// a change, so it answers with what it did rather than running out.
pub(crate) const PRUNE_BUDGET: Duration = Duration::from_secs(130);
/// How long a prune may take at all.
pub(crate) const PRUNE_TIMEOUT: Duration = Duration::from_secs(140);
/// The networks every engine has.
const BUILT_IN_NETWORKS: [&str; 3] = ["bridge", "host", "none"];
/// The label an engine puts on a volume a container made without naming it.
const ANONYMOUS: &str = "com.docker.volume.anonymous";

fn item(kind: EnginePruneKind, id: String, name: String, size_bytes: u64) -> wire::EnginePruneItem {
    wire::EnginePruneItem {
        kind: kind.into(),
        id,
        name,
        size_bytes,
    }
}

fn running(container: &ContainerSummary) -> bool {
    matches!(
        container.state,
        Some(
            ContainerSummaryStateEnum::RUNNING
                | ContainerSummaryStateEnum::RESTARTING
                | ContainerSummaryStateEnum::PAUSED
        )
    )
}

fn positive(value: Option<i64>) -> u64 {
    value
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

/// The volumes' sizes and the build cache records neither in use nor
/// shared, from the engine's disk use in either API shape.
fn from_disk_usage(df: &Value) -> (HashMap<String, u64>, Vec<wire::EnginePruneItem>) {
    let list = |legacy: &str, current: &str| -> Vec<Value> {
        df[legacy]
            .as_array()
            .or_else(|| df[current]["Items"].as_array())
            .cloned()
            .unwrap_or_default()
    };
    let sizes = list("Volumes", "VolumeUsage")
        .iter()
        .filter_map(|volume| {
            let size = volume["UsageData"]["Size"].as_u64()?;
            Some((volume["Name"].as_str()?.to_owned(), size))
        })
        .collect();
    let cache = list("BuildCache", "BuildCacheUsage")
        .iter()
        .filter(|record| record["InUse"] != true && record["Shared"] != true)
        .filter_map(|record| {
            let id = record["ID"].as_str()?.to_owned();
            let name = record["Description"]
                .as_str()
                .filter(|description| !description.is_empty())
                .or_else(|| record["Type"].as_str())
                .unwrap_or("build cache")
                .chars()
                .take(120)
                .collect();
            let size = record["Size"].as_u64().unwrap_or(0);
            Some(item(EnginePruneKind::BuildCache, id, name, size))
        })
        .collect();
    (sizes, cache)
}

/// What pruning an engine would remove, by kind then name.
pub(crate) async fn preview(
    client: &Docker,
    socket: &Path,
    installation: &str,
    choices: wire::EnginePruneChoices,
) -> Result<Vec<wire::EnginePruneItem>, PanelError> {
    let installs = |labels: Option<&HashMap<String, String>>| {
        labels
            .and_then(|labels| labels.get(COMPOSE_PROJECT))
            .map(String::as_str)
            == Some(installation)
    };
    let containers = client
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .size(true)
                .build(),
        ))
        .await
        .map_err(|error| failure(&error))?;
    let mut images_used = HashSet::new();
    let mut volumes_used = HashSet::new();
    let mut networks_used = HashSet::new();
    let mut items = Vec::new();
    for container in &containers {
        images_used.extend(container.image_id.clone());
        for mount in container.mounts.iter().flatten() {
            if mount.typ.as_deref() == Some("volume") {
                volumes_used.extend(mount.name.clone());
            }
        }
        if let Some(networks) = container
            .network_settings
            .as_ref()
            .and_then(|settings| settings.networks.as_ref())
        {
            networks_used.extend(networks.keys().cloned());
        }
        if !running(container) && !installs(container.labels.as_ref()) {
            let name = container
                .names
                .iter()
                .flatten()
                .next()
                .map(|name| name.trim_start_matches('/').to_owned())
                .unwrap_or_default();
            let id = container.id.clone().unwrap_or_default();
            items.push(item(
                EnginePruneKind::Container,
                id,
                name,
                positive(container.size_rw),
            ));
        }
    }

    let images = client
        .list_images(Some(
            ListImagesOptionsBuilder::default()
                .shared_size(true)
                .build(),
        ))
        .await
        .map_err(|error| failure(&error))?;
    for image in images {
        let tags = named(image.repo_tags);
        if images_used.contains(&image.id) || (!tags.is_empty() && !choices.tagged_images) {
            continue;
        }
        let size = positive(Some(image.size));
        let unique = match u64::try_from(image.shared_size) {
            Ok(shared) => size.saturating_sub(shared),
            Err(_) => size,
        };
        let name = tags.into_iter().next().unwrap_or_else(|| image.id.clone());
        items.push(item(EnginePruneKind::Image, image.id, name, unique));
    }

    let (volume_sizes, build_cache) = from_disk_usage(&engine_disk::data_usage(socket).await?);
    let volumes = client
        .list_volumes(None::<ListVolumesOptions>)
        .await
        .map_err(|error| failure(&error))?
        .volumes
        .unwrap_or_default();
    for volume in volumes {
        let anonymous = volume.labels.contains_key(ANONYMOUS);
        if volumes_used.contains(&volume.name)
            || installs(Some(&volume.labels))
            || (!anonymous && !choices.named_volumes)
        {
            continue;
        }
        let size = volume_sizes.get(&volume.name).copied().unwrap_or(0);
        items.push(item(
            EnginePruneKind::Volume,
            volume.name.clone(),
            volume.name,
            size,
        ));
    }

    let networks = client
        .list_networks(None::<ListNetworksOptions>)
        .await
        .map_err(|error| failure(&error))?;
    for network in networks {
        let name = network.name.clone().unwrap_or_default();
        let driver = network.driver.as_deref().unwrap_or_default();
        if BUILT_IN_NETWORKS.contains(&name.as_str())
            || matches!(driver, "host" | "null")
            || network
                .scope
                .as_deref()
                .is_some_and(|scope| scope != "local")
            || networks_used.contains(&name)
            || installs(network.labels.as_ref())
        {
            continue;
        }
        items.push(item(
            EnginePruneKind::Network,
            network.id.unwrap_or_default(),
            name,
            0,
        ));
    }

    items.extend(build_cache);
    items.sort_by(|left, right| (left.kind, &left.name).cmp(&(right.kind, &right.name)));
    Ok(items)
}

/// Removes one container, image, volume or network the preview listed.
async fn remove(client: &Docker, kind: EnginePruneKind, id: &str) -> Result<(), PanelError> {
    let removed = match kind {
        EnginePruneKind::Container => {
            let options = RemoveContainerOptionsBuilder::default().build();
            client.remove_container(id, Some(options)).await
        }
        // An image several tags name goes only with force; no container
        // uses it, so force removes nothing else.
        EnginePruneKind::Image => {
            let options = RemoveImageOptionsBuilder::default().force(true).build();
            client
                .remove_image(id, Some(options), None)
                .await
                .map(|_| ())
        }
        EnginePruneKind::Volume => {
            let options = RemoveVolumeOptionsBuilder::default().build();
            client.remove_volume(id, Some(options)).await
        }
        EnginePruneKind::Network => client.remove_network(id).await,
        EnginePruneKind::BuildCache | EnginePruneKind::Unspecified => {
            return Err(PanelError::invalid_argument("name what to prune"));
        }
    };
    removed.map_err(|error| failure(&error))
}

/// Removes the requested items a fresh preview with the same choices
/// still lists, in the order asked, starting none after `deadline`;
/// returns each outcome and what the items that went freed.
pub(crate) async fn prune(
    client: &Docker,
    socket: &Path,
    installation: &str,
    choices: wire::EnginePruneChoices,
    items: Vec<wire::EnginePruneItem>,
    deadline: Instant,
) -> Result<(Vec<wire::EnginePruneOutcome>, u64), PanelError> {
    if items.len() > MOST_ITEMS {
        return Err(PanelError::invalid_argument(format!(
            "prune at most {MOST_ITEMS} items at a time"
        )));
    }
    let listed: HashMap<(i32, String), wire::EnginePruneItem> =
        preview(client, socket, installation, choices)
            .await?
            .into_iter()
            .map(|item| ((item.kind, item.id.clone()), item))
            .collect();
    let mut outcomes: Vec<(wire::EnginePruneItem, Option<PanelError>)> = Vec::new();
    let mut cache = Vec::new();
    for requested in items {
        let Some(current) = listed.get(&(requested.kind, requested.id.clone())) else {
            let gone = PanelError::precondition_failed(format!(
                "{} is no longer what pruning would remove",
                if requested.name.is_empty() {
                    &requested.id
                } else {
                    &requested.name
                }
            ));
            outcomes.push((requested, Some(gone)));
            continue;
        };
        if Instant::now() >= deadline {
            let late = PanelError::deadline_exceeded("pruning ran out of time before this item");
            outcomes.push((current.clone(), Some(late)));
            continue;
        }
        let kind = EnginePruneKind::try_from(current.kind).unwrap_or(EnginePruneKind::Unspecified);
        if kind == EnginePruneKind::BuildCache {
            cache.push(outcomes.len());
            outcomes.push((current.clone(), None));
            continue;
        }
        let removed = remove(client, kind, &current.id).await;
        outcomes.push((current.clone(), removed.err()));
    }
    if !cache.is_empty() {
        let ids: Vec<String> = cache.iter().map(|&at| outcomes[at].0.id.clone()).collect();
        let filters = HashMap::from([("id", ids)]);
        let pruned = client
            .prune_build(Some(
                PruneBuildOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
            .map_err(|error| failure(&error));
        let deleted: HashSet<String> = match &pruned {
            Ok(answer) => answer.caches_deleted.iter().flatten().cloned().collect(),
            Err(_) => HashSet::new(),
        };
        for at in cache {
            if !deleted.contains(&outcomes[at].0.id) {
                outcomes[at].1 = Some(match &pruned {
                    Err(error) => error.clone(),
                    Ok(_) => PanelError::conflict("the build cache record is in use"),
                });
            }
        }
    }
    let reclaimed = outcomes
        .iter()
        .filter(|(_, error)| error.is_none())
        .map(|(item, _)| item.size_bytes)
        .sum();
    Ok((
        outcomes
            .into_iter()
            .map(|(item, error)| wire::EnginePruneOutcome {
                item: Some(item),
                error: error.as_ref().map(Into::into),
            })
            .collect(),
        reclaimed,
    ))
}
