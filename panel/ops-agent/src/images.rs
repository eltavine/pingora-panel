//! The images on the engines the agent reaches (ADR 0031): listed,
//! inspected without their environment or command line, and removed unless
//! the panel's own installation uses them.

use crate::containers::{answer, failure, time, Engines, ACTION_TIMEOUT, COMPOSE_PROJECT};
use bollard::{
    models::{ImageInspect, ImageSummary},
    query_parameters::{
        ListContainersOptionsBuilder, ListImagesOptionsBuilder, RemoveImageOptionsBuilder,
    },
    Docker,
};
use panel_contracts::ops::v1::{self as wire, images_server::Images};
use panel_errors::PanelError;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::{Request, Response, Status};

/// An image's ID, a prefix of one, or a reference, by the characters
/// references allow. Each part between slashes is a name, never `.` or
/// `..`, so a reference stays within the engine's image paths.
fn reference(value: &str) -> Result<&str, PanelError> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 512
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if valid {
        Ok(value)
    } else {
        Err(PanelError::invalid_argument(format!(
            "`{value}` is not an image's ID or reference"
        )))
    }
}

/// References without the `<none>` placeholders the engine gives an image
/// nothing names.
fn named(references: Vec<String>) -> Vec<String> {
    references
        .into_iter()
        .filter(|reference| !reference.starts_with("<none>"))
        .collect()
}

fn image(summary: ImageSummary, used: &HashMap<String, u32>) -> wire::Image {
    wire::Image {
        containers: used.get(&summary.id).copied().unwrap_or(0),
        id: summary.id,
        tags: named(summary.repo_tags),
        digests: named(summary.repo_digests),
        created: u64::try_from(summary.created)
            .ok()
            .map(|seconds| (UNIX_EPOCH + Duration::from_secs(seconds)).into()),
        size_bytes: u64::try_from(summary.size).unwrap_or(0),
        labels: summary.labels.into_iter().collect(),
    }
}

/// An image's configuration, without its environment or command line.
fn detail(inspected: ImageInspect, containers: u32) -> wire::ImageDetail {
    let config = inspected.config.unwrap_or_default();
    let sorted = |values: Option<Vec<String>>| {
        let mut values = values.unwrap_or_default();
        values.sort();
        values
    };
    wire::ImageDetail {
        image: Some(wire::Image {
            id: inspected.id.unwrap_or_default(),
            tags: named(inspected.repo_tags.unwrap_or_default()),
            digests: named(inspected.repo_digests.unwrap_or_default()),
            created: time(inspected.created.as_deref()).map(Into::into),
            size_bytes: inspected
                .size
                .and_then(|size| u64::try_from(size).ok())
                .unwrap_or(0),
            containers,
            labels: config.labels.unwrap_or_default().into_iter().collect(),
        }),
        architecture: inspected.architecture.unwrap_or_default(),
        variant: inspected.variant.unwrap_or_default(),
        os: inspected.os.unwrap_or_default(),
        author: inspected.author.unwrap_or_default(),
        comment: inspected.comment.unwrap_or_default(),
        user: config.user.unwrap_or_default(),
        working_directory: config.working_dir.unwrap_or_default(),
        exposed_ports: sorted(config.exposed_ports),
        volumes: sorted(config.volumes),
        stop_signal: config.stop_signal.unwrap_or_default(),
        layers: inspected
            .root_fs
            .and_then(|root| root.layers)
            .map_or(0, |layers| u32::try_from(layers.len()).unwrap_or(u32::MAX)),
    }
}

/// Whether an image answers a search of its tags and ID.
fn matches(image: &wire::Image, search: &str) -> bool {
    let search = search.trim().to_lowercase();
    search.is_empty()
        || image.id.to_lowercase().contains(&search)
        || image
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(&search))
}

/// The images to panel-api.
pub(crate) struct ImageService {
    engines: Arc<Engines>,
    /// The Compose project of the panel's own installation.
    installation: String,
}

impl ImageService {
    pub(crate) fn new(engines: Arc<Engines>, installation: String) -> Self {
        Self {
            engines,
            installation,
        }
    }

    /// How many containers, running or not, each image has, and whether the
    /// installation's use it.
    async fn uses(
        &self,
        client: &Docker,
    ) -> Result<(HashMap<String, u32>, Vec<String>), PanelError> {
        let options = ListContainersOptionsBuilder::default().all(true).build();
        let mut counts = HashMap::new();
        let mut installation = Vec::new();
        for container in client
            .list_containers(Some(options))
            .await
            .map_err(|error| failure(&error))?
        {
            let Some(image) = container.image_id else {
                continue;
            };
            let project = container
                .labels
                .as_ref()
                .and_then(|labels| labels.get(COMPOSE_PROJECT));
            if project == Some(&self.installation) {
                installation.push(image.clone());
            }
            *counts.entry(image).or_insert(0) += 1;
        }
        Ok((counts, installation))
    }

    async fn listed(
        &self,
        request: &wire::ImagesListRequest,
    ) -> Result<Vec<wire::Image>, PanelError> {
        let client = self.engines.enabled(&request.engine)?;
        let summaries = client
            .list_images(Some(ListImagesOptionsBuilder::default().all(false).build()))
            .await
            .map_err(|error| failure(&error))?;
        let (used, _) = self.uses(&client).await?;
        let mut images: Vec<wire::Image> = summaries
            .into_iter()
            .map(|summary| image(summary, &used))
            .filter(|image| matches(image, &request.search))
            .collect();
        images.sort_by(
            |left, right| match (left.tags.first(), right.tags.first()) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => right
                    .created
                    .map(|time| time.seconds)
                    .cmp(&left.created.map(|time| time.seconds)),
            },
        );
        Ok(images)
    }

    async fn inspected(
        &self,
        request: &wire::ImagesInspectRequest,
    ) -> Result<wire::ImageDetail, PanelError> {
        let name = reference(&request.image)?;
        let client = self.engines.enabled(&request.engine)?;
        let inspected = client
            .inspect_image(name)
            .await
            .map_err(|error| failure(&error))?;
        let (used, _) = self.uses(&client).await?;
        let containers = inspected
            .id
            .as_ref()
            .and_then(|id| used.get(id))
            .copied()
            .unwrap_or(0);
        Ok(detail(inspected, containers))
    }

    async fn removed(
        &self,
        request: &wire::ImagesRemoveRequest,
    ) -> Result<wire::ImagesRemoveResponse, PanelError> {
        let name = reference(&request.image)?;
        let client = self
            .engines
            .enabled(&request.engine)?
            .with_timeout(ACTION_TIMEOUT);
        let id = client
            .inspect_image(name)
            .await
            .map_err(|error| failure(&error))?
            .id
            .unwrap_or_default();
        let (_, installation) = self.uses(&client).await?;
        if installation.contains(&id) {
            return Err(PanelError::precondition_failed(format!(
                "{name} is used by the panel's installation; manage it with Compose"
            )));
        }
        let options = RemoveImageOptionsBuilder::default()
            .force(request.force)
            .build();
        let removed = client
            .remove_image(name, Some(options), None)
            .await
            .map_err(|error| failure(&error))?;
        Ok(wire::ImagesRemoveResponse {
            id,
            untagged: removed
                .iter()
                .filter_map(|item| item.untagged.clone())
                .collect(),
            deleted: removed
                .into_iter()
                .filter_map(|item| item.deleted)
                .collect(),
            error: None,
        })
    }
}

#[tonic::async_trait]
impl Images for ImageService {
    async fn list(
        &self,
        request: Request<wire::ImagesListRequest>,
    ) -> Result<Response<wire::ImagesListResponse>, Status> {
        let (images, error) = answer(self.listed(request.get_ref()).await);
        Ok(Response::new(wire::ImagesListResponse {
            observed_at: images.as_ref().map(|_| SystemTime::now().into()),
            images: images.unwrap_or_default(),
            error,
        }))
    }

    async fn inspect(
        &self,
        request: Request<wire::ImagesInspectRequest>,
    ) -> Result<Response<wire::ImagesInspectResponse>, Status> {
        let (detail, error) = answer(self.inspected(request.get_ref()).await);
        Ok(Response::new(wire::ImagesInspectResponse { detail, error }))
    }

    async fn remove(
        &self,
        request: Request<wire::ImagesRemoveRequest>,
    ) -> Result<Response<wire::ImagesRemoveResponse>, Status> {
        let request = request.into_inner();
        let result = tokio::time::timeout(ACTION_TIMEOUT, self.removed(&request))
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not finish in time",
                ))
            });
        Ok(Response::new(match result {
            Ok(removed) => {
                tracing::info!(
                    event = "image_removed",
                    engine = %request.engine,
                    image = %request.image,
                    deleted = removed.deleted.len(),
                );
                removed
            }
            Err(error) => {
                tracing::warn!(
                    event = "image_removal_refused",
                    engine = %request.engine,
                    image = %request.image,
                    error_code = %error.code,
                );
                wire::ImagesRemoveResponse {
                    error: Some((&error).into()),
                    ..wire::ImagesRemoveResponse::default()
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_engine::{engine, engine_with, engines, Calls};
    use std::path::PathBuf;

    fn service(socket: PathBuf, installation: &str) -> ImageService {
        ImageService::new(engines(socket, None), installation.into())
    }

    async fn listed(service: &ImageService, search: &str) -> wire::ImagesListResponse {
        service
            .list(Request::new(wire::ImagesListRequest {
                context: None,
                engine: "docker".into(),
                search: search.into(),
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn images_are_listed_with_the_containers_made_from_them() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(engine(directory.path()).await, "pingora-panel");
        let every = listed(&service, "").await;
        assert!(every.error.is_none(), "{:?}", every.error);
        assert!(every.observed_at.is_some());
        let tags: Vec<_> = every
            .images
            .iter()
            .map(|image| image.tags.clone())
            .collect();
        assert_eq!(
            tags,
            [
                vec!["nginx:1.27".to_owned()],
                vec!["redis:7".to_owned()],
                vec![]
            ],
            "by tag, and an image nothing names last"
        );
        let nginx = &every.images[0];
        assert_eq!((nginx.id.as_str(), nginx.containers), ("sha256:aa", 1));
        assert_eq!(nginx.digests, ["nginx@sha256:d1"]);
        assert_eq!(nginx.size_bytes, 50_000_000);
        assert_eq!(nginx.created.unwrap().seconds, 1_800_000_000);
        assert!(every.images[2].digests.is_empty());
        assert_eq!(every.images[2].containers, 0);

        let found = listed(&service, "REDIS").await;
        assert_eq!(found.images.len(), 1);
    }

    #[tokio::test]
    async fn images_are_inspected_without_their_environment_or_command() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(engine(directory.path()).await, "pingora-panel");
        let inspect = |image: &str| {
            service.inspect(Request::new(wire::ImagesInspectRequest {
                context: None,
                engine: "docker".into(),
                image: image.into(),
            }))
        };
        let detail = inspect("nginx:1.27")
            .await
            .unwrap()
            .into_inner()
            .detail
            .unwrap();
        assert!(!format!("{detail:?}").contains("hunter2"), "{detail:?}");
        assert_eq!(detail.image.as_ref().unwrap().containers, 1);
        assert_eq!(
            (
                detail.architecture.as_str(),
                detail.os.as_str(),
                detail.user.as_str()
            ),
            ("amd64", "linux", "nginx")
        );
        assert_eq!(detail.exposed_ports, ["443/tcp", "80/tcp"]);
        assert_eq!(detail.volumes, ["/var/cache/nginx"]);
        assert_eq!((detail.layers, detail.stop_signal.as_str()), (2, "SIGQUIT"));

        let missing = inspect("ghost:1").await.unwrap().into_inner();
        assert_eq!(missing.error.unwrap().code, "NOT_FOUND");
        let escaping = inspect("../containers/b2/json").await.unwrap().into_inner();
        assert_eq!(escaping.error.unwrap().code, "INVALID_ARGUMENT");
    }

    async fn remove(
        service: &ImageService,
        image: &str,
        force: bool,
    ) -> wire::ImagesRemoveResponse {
        service
            .remove(Request::new(wire::ImagesRemoveRequest {
                context: None,
                engine: "docker".into(),
                image: image.into(),
                force,
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn images_are_removed_unless_the_installation_uses_them() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let socket = engine_with(directory.path(), calls.clone()).await;
        let images = service(socket.clone(), "pingora-panel");

        let stopped = remove(&images, "redis:7", false).await;
        assert_eq!(stopped.error.unwrap().code, "CONFLICT");
        let forced = remove(&images, "redis:7", true).await;
        assert!(forced.error.is_none(), "{:?}", forced.error);
        assert_eq!(forced.id, "sha256:bb");
        assert_eq!(forced.untagged, ["redis:7"]);
        assert_eq!(forced.deleted, ["sha256:bb"]);
        let running = remove(&images, "nginx:1.27", true).await;
        assert_eq!(running.error.unwrap().code, "CONFLICT");

        let installation = service(socket, "shop");
        let refused = remove(&installation, "nginx:1.27", true).await;
        assert_eq!(refused.error.unwrap().code, "PRECONDITION_FAILED");
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "remove-image redis:7 force=false",
                "remove-image redis:7 force=true",
                "remove-image nginx:1.27 force=true"
            ],
            "the installation's image never reaches the engine"
        );
    }

    #[test]
    fn references_stay_within_the_image_paths() {
        for valid in [
            "nginx",
            "nginx:1.27",
            "sha256:4f1c2a9b",
            "ghcr.io/example/app:2.3",
            "registry.example:5000/team/app@sha256:abc",
        ] {
            assert_eq!(reference(valid).unwrap(), valid);
        }
        for invalid in [
            "",
            "../containers/b2/kill",
            "nginx/../../containers/b2",
            "/nginx",
            "nginx//latest",
            "nginx:1.27?force=1",
            "-nginx",
            "nginx latest",
        ] {
            assert!(reference(invalid).is_err(), "{invalid}");
        }
    }
}
