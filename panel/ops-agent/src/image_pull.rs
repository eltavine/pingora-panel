//! Pulling images (ADR 0031): the engine downloads them from their
//! registries, and the agent passes on how far each layer got.

use crate::{containers::failure, images};
use bollard::{
    auth::DockerCredentials, errors::Error as EngineError, models::CreateImageInfo,
    query_parameters::CreateImageOptionsBuilder, Docker,
};
use futures_util::{stream::BoxStream, StreamExt};
use panel_contracts::ops::v1::{self as wire, ImageLayerState};
use panel_errors::PanelError;
use std::time::Duration;
use tokio::{
    sync::{mpsc, OwnedSemaphorePermit},
    time::{Instant, MissedTickBehavior},
};

/// How many images the agent pulls at once.
pub(crate) const PULLS: usize = 4;
/// The longest a pull may take.
const PULL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How often progress is passed on at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// How often progress is passed on at least, so that nothing between the
/// agent and whoever asked gives up on a quiet pull.
const HEARTBEAT: Duration = Duration::from_secs(10);

/// A reference the engine can pull, split as its API takes it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Pullable {
    /// Such as `nginx` or `ghcr.io/example/app`.
    pub(crate) name: String,
    /// A tag, or a digest such as `sha256:…`.
    pub(crate) tag: String,
    /// Such as `ghcr.io`; none for Docker Hub.
    pub(crate) registry: Option<String>,
}

impl Pullable {
    /// The reference as the engine names what it pulled.
    pub(crate) fn reference(&self) -> String {
        if self.tag.contains(':') {
            format!("{}@{}", self.name, self.tag)
        } else {
            format!("{}:{}", self.name, self.tag)
        }
    }
}

fn lowercase_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

/// A path component: lowercase letters and digits, separated by one `.`
/// or `_`, two `_`, or any number of `-`.
fn component(value: &str) -> bool {
    let bytes = value.as_bytes();
    let (Some(&first), Some(&last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    if !lowercase_alphanumeric(first) || !lowercase_alphanumeric(last) {
        return false;
    }
    value
        .split(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        .filter(|separator| !separator.is_empty())
        .all(|separator| {
            matches!(separator, "." | "_" | "__") || separator.bytes().all(|byte| byte == b'-')
        })
}

/// A registry's host name, with a port if any.
fn registry(value: &str) -> bool {
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (value, None),
    };
    let label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    host.split('.').all(label)
        && port.is_none_or(|port| {
            !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn tag(value: &str) -> bool {
    value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric() || first == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// A digest such as `sha256:` and at least 32 hexadecimal digits.
fn digest(value: &str) -> bool {
    let Some((algorithm, hex)) = value.split_once(':') else {
        return false;
    };
    algorithm
        .bytes()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && algorithm.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'+' | b'.' | b'_' | b'-')
        })
        && hex.len() >= 32
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A reference as the distribution specification writes one:
/// `[registry/]path[:tag][@digest]`. A digest names what is pulled, a tag
/// otherwise, and `latest` when there is neither, so that the engine never
/// pulls every tag of a name.
pub(crate) fn pullable(value: &str) -> Result<Pullable, PanelError> {
    let value = value.trim();
    let refused = || {
        PanelError::invalid_argument(format!(
            "`{value}` is not an image reference such as nginx:1.27"
        ))
    };
    if value.is_empty() || value.len() > 512 {
        return Err(refused());
    }
    let (named, pinned) = match value.split_once('@') {
        Some((named, pinned)) if digest(pinned) => (named, Some(pinned)),
        Some(_) => return Err(refused()),
        None => (value, None),
    };
    // A colon after the last slash starts a tag; one before it, a port.
    let last = named.rfind('/').map_or(0, |slash| slash + 1);
    let (name, tagged) = match named[last..].rfind(':') {
        Some(colon) => (&named[..last + colon], Some(&named[last + colon + 1..])),
        None => (named, None),
    };
    if tagged.is_some_and(|tagged| !tag(tagged)) || name.len() > 255 {
        return Err(refused());
    }
    let mut parts: Vec<&str> = name.split('/').collect();
    let host = (parts.len() > 1 && (parts[0].contains(['.', ':']) || parts[0] == "localhost"))
        .then(|| parts.remove(0));
    if host.is_some_and(|host| !registry(host)) || !parts.iter().all(|part| component(part)) {
        return Err(refused());
    }
    Ok(Pullable {
        name: name.to_owned(),
        tag: pinned.or(tagged).unwrap_or("latest").to_owned(),
        registry: host.map(str::to_owned),
    })
}

/// A platform such as `linux/arm64` or `linux/arm/v7`.
pub(crate) fn platform(value: &str) -> Result<&str, PanelError> {
    let value = value.trim();
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| lowercase_alphanumeric(byte) || matches!(byte, b'_' | b'.' | b'-'))
    };
    let parts = value.split('/').count();
    if value.is_empty()
        || (value.len() <= 64 && (2..=3).contains(&parts) && value.split('/').all(part))
    {
        Ok(value)
    } else {
        Err(PanelError::invalid_argument(format!(
            "`{value}` is not a platform such as linux/arm64"
        )))
    }
}

/// Credentials for the registry the reference names.
pub(crate) fn credentials(
    wanted: &Pullable,
    given: Option<wire::RegistryCredentials>,
) -> Option<DockerCredentials> {
    let given = given.filter(|given| !given.username.is_empty() || !given.password.is_empty())?;
    Some(DockerCredentials {
        username: Some(given.username),
        password: Some(given.password),
        serveraddress: Some(
            wanted
                .registry
                .clone()
                .unwrap_or_else(|| "docker.io".into()),
        ),
        ..DockerCredentials::default()
    })
}

/// Why a pull failed, from the engine's words. A registry that wants a
/// sign-in is a permission the panel lacks, never the panel's own session.
pub(crate) fn pull_failure(error: &EngineError) -> PanelError {
    let message = match error {
        EngineError::DockerStreamError { error } => error.clone(),
        EngineError::DockerResponseServerError {
            status_code: 404,
            message,
        } => return PanelError::not_found(message.clone()),
        EngineError::DockerResponseServerError {
            status_code: 400,
            message,
        } => return PanelError::invalid_argument(message.clone()),
        EngineError::DockerResponseServerError { message, .. } => message.clone(),
        other => return failure(other),
    };
    let lowered = message.to_lowercase();
    if lowered.contains("toomanyrequests") || lowered.contains("rate limit") {
        PanelError::resource_exhausted(message)
    } else if ["unauthorized", "authentication required", "denied"]
        .iter()
        .any(|word| lowered.contains(word))
    {
        PanelError::permission_denied(message)
    } else if [
        "not found",
        "manifest unknown",
        "does not exist",
        "no matching manifest",
    ]
    .iter()
    .any(|words| lowered.contains(words))
    {
        PanelError::not_found(message)
    } else {
        PanelError::unavailable(format!("the engine: {message}"))
    }
}

/// How far a pull got, from the engine's messages.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    layers: Vec<wire::ImageLayerProgress>,
    digest: Option<String>,
    updated: Option<bool>,
    changed: bool,
}

/// A layer's short ID, as the engine names layers in its messages; other
/// messages carry a tag there.
fn layer(id: &str) -> bool {
    id.len() >= 12 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Progress {
    pub(crate) fn read(&mut self, info: CreateImageInfo) {
        let status = info.status.unwrap_or_default();
        if let Some(digest) = status.strip_prefix("Digest: ") {
            self.digest = Some(digest.trim().to_owned());
            return;
        }
        if status.starts_with("Status: Downloaded newer image") {
            self.updated = Some(true);
            return;
        }
        if status.starts_with("Status: Image is up to date") {
            self.updated = Some(false);
            return;
        }
        let Some(id) = info.id.filter(|id| layer(id)) else {
            return;
        };
        let state = match status.as_str() {
            "Pulling fs layer" | "Waiting" => ImageLayerState::Waiting,
            "Verifying Checksum" | "Download complete" => ImageLayerState::Downloaded,
            "Pull complete" => ImageLayerState::Complete,
            "Already exists" => ImageLayerState::Exists,
            status if status.starts_with("Downloading") => ImageLayerState::Downloading,
            status if status.starts_with("Extracting") => ImageLayerState::Extracting,
            status if status.starts_with("Retrying") => ImageLayerState::Waiting,
            _ => return,
        };
        let (current, total) = info.progress_detail.map_or((0, 0), |detail| {
            let bytes = |value: Option<i64>| value.and_then(|value| u64::try_from(value).ok());
            (
                bytes(detail.current).unwrap_or(0),
                bytes(detail.total).unwrap_or(0),
            )
        });
        let at = match self.layers.iter().position(|known| known.id == id) {
            Some(at) => at,
            None => {
                self.layers.push(wire::ImageLayerProgress {
                    id,
                    ..wire::ImageLayerProgress::default()
                });
                self.layers.len() - 1
            }
        };
        let known = &mut self.layers[at];
        known.state = state.into();
        match state {
            ImageLayerState::Downloading | ImageLayerState::Extracting => {
                known.current_bytes = current;
                if total > 0 {
                    known.total_bytes = total;
                }
            }
            ImageLayerState::Downloaded | ImageLayerState::Complete => {
                known.current_bytes = known.total_bytes;
            }
            _ => {}
        }
        self.changed = true;
    }

    fn message(&self) -> wire::ImagesPullResponse {
        wire::ImagesPullResponse {
            layers: self.layers.clone(),
            ..wire::ImagesPullResponse::default()
        }
    }

    /// Whether the engine downloaded anything, when it does not say.
    fn updated(&self) -> bool {
        self.updated.unwrap_or_else(|| {
            self.layers
                .iter()
                .any(|layer| layer.state != i32::from(ImageLayerState::Exists))
        })
    }
}

/// Pulls `wanted`, holding `permit` until the pull ends; the stream's last
/// message says what was pulled, with how many containers of the
/// `installation`'s project use it, or why not.
pub(crate) fn pull(
    client: Docker,
    wanted: Pullable,
    platform: String,
    credentials: Option<DockerCredentials>,
    installation: String,
    permit: OwnedSemaphorePermit,
) -> BoxStream<'static, wire::ImagesPullResponse> {
    let (sender, mut receiver) = mpsc::channel(4);
    tokio::spawn(async move {
        let _pulling = permit;
        let options = CreateImageOptionsBuilder::default()
            .from_image(&wanted.name)
            .tag(&wanted.tag)
            .platform(&platform)
            .build();
        let mut engine = client.create_image(Some(options), None, credentials);
        let mut progress = Progress::default();
        let mut ticks = tokio::time::interval(PROGRESS_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut sent_at = Instant::now();
        let limit = tokio::time::sleep(PULL_TIMEOUT);
        tokio::pin!(limit);
        let read = loop {
            tokio::select! {
                // Nobody waits for it any more: dropping what the engine
                // sends cancels the pull.
                () = sender.closed() => return,
                () = &mut limit => {
                    break Err(PanelError::deadline_exceeded("the pull took longer than 30 minutes"));
                }
                next = engine.next() => match next {
                    Some(Ok(info)) => progress.read(info),
                    Some(Err(error)) => break Err(pull_failure(&error)),
                    None => break Ok(()),
                },
                _ = ticks.tick() => {
                    if progress.changed || sent_at.elapsed() >= HEARTBEAT {
                        progress.changed = false;
                        sent_at = Instant::now();
                        if sender.send(progress.message()).await.is_err() {
                            return;
                        }
                    }
                }
            }
        };
        drop(engine);
        // Where the layers ended goes out on its own before the last
        // message, for a pull that ended before a tick sent it.
        if progress.changed && sender.send(progress.message()).await.is_err() {
            return;
        }
        let reference = wanted.reference();
        let last = match read {
            Ok(()) => match images::listed(&client, &reference, &installation).await {
                Ok(image) => {
                    tracing::info!(
                        event = "image_pulled",
                        image = %reference,
                        updated = progress.updated(),
                    );
                    wire::ImagesPullResponse {
                        pulled: Some(wire::ImagePulled {
                            image: Some(image),
                            digest: progress.digest.clone().unwrap_or_default(),
                            updated: progress.updated(),
                        }),
                        ..progress.message()
                    }
                }
                Err(error) => refused(&reference, &progress, &error),
            },
            Err(error) => refused(&reference, &progress, &error),
        };
        let _ = sender.send(last).await;
    });
    futures_util::stream::poll_fn(move |context| receiver.poll_recv(context)).boxed()
}

fn refused(reference: &str, progress: &Progress, error: &PanelError) -> wire::ImagesPullResponse {
    tracing::warn!(
        event = "image_pull_refused",
        image = %reference,
        error_code = %error.code,
    );
    wire::ImagesPullResponse {
        error: Some(error.into()),
        ..progress.message()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::ProgressDetail;

    fn wanted(name: &str, tag: &str, registry: Option<&str>) -> Pullable {
        Pullable {
            name: name.into(),
            tag: tag.into(),
            registry: registry.map(Into::into),
        }
    }

    #[test]
    fn references_follow_the_distribution_grammar() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        for (value, expected) in [
            ("nginx", wanted("nginx", "latest", None)),
            ("nginx:1.27", wanted("nginx", "1.27", None)),
            (
                "library/nginx:1.27-alpine",
                wanted("library/nginx", "1.27-alpine", None),
            ),
            (
                "ghcr.io/example/app:2.3",
                wanted("ghcr.io/example/app", "2.3", Some("ghcr.io")),
            ),
            (
                "localhost:5000/team/app",
                wanted("localhost:5000/team/app", "latest", Some("localhost:5000")),
            ),
            (
                "registry.example:8443/a__b/c-d.e:v1",
                wanted(
                    "registry.example:8443/a__b/c-d.e",
                    "v1",
                    Some("registry.example:8443"),
                ),
            ),
        ] {
            assert_eq!(pullable(value).unwrap(), expected, "{value}");
        }
        let pinned = pullable(&format!("ghcr.io/example/app:2.3@{digest}")).unwrap();
        assert_eq!(pinned.tag, digest);
        assert_eq!(pinned.reference(), format!("ghcr.io/example/app@{digest}"));
        assert_eq!(pullable("nginx").unwrap().reference(), "nginx:latest");

        for value in [
            "",
            "Nginx",
            "nginx:",
            "nginx:-1",
            "nginx@sha256:short",
            "../nginx",
            "a//b",
            "a/./b",
            "app_",
            "a...b",
            "nginx:1.27 --rm",
            "-registry.example/app",
            "registry.example:port/app",
        ] {
            assert!(pullable(value).is_err(), "{value}");
        }
    }

    #[test]
    fn platforms_are_two_or_three_parts() {
        for value in ["", "linux/amd64", "linux/arm/v7", "windows/amd64"] {
            assert!(platform(value).is_ok(), "{value}");
        }
        for value in ["linux", "linux/arm/v7/x", "Linux/amd64", "linux/amd64;"] {
            assert!(platform(value).is_err(), "{value}");
        }
    }

    #[test]
    fn credentials_name_the_registry() {
        let given = |username: &str, password: &str| wire::RegistryCredentials {
            username: username.into(),
            password: password.into(),
        };
        let private = pullable("ghcr.io/example/app:2.3").unwrap();
        let sent = credentials(&private, Some(given("ci", "token"))).unwrap();
        assert_eq!(sent.serveraddress.as_deref(), Some("ghcr.io"));
        assert_eq!(sent.username.as_deref(), Some("ci"));
        let hub = pullable("nginx").unwrap();
        assert_eq!(
            credentials(&hub, Some(given("ci", "token")))
                .unwrap()
                .serveraddress
                .as_deref(),
            Some("docker.io")
        );
        assert!(credentials(&hub, Some(given("", ""))).is_none());
        assert!(credentials(&hub, None).is_none());
    }

    fn info(id: &str, status: &str, current: i64, total: i64) -> CreateImageInfo {
        CreateImageInfo {
            id: Some(id.into()),
            status: Some(status.into()),
            progress_detail: Some(ProgressDetail {
                current: Some(current),
                total: Some(total),
            }),
            ..CreateImageInfo::default()
        }
    }

    #[test]
    fn progress_follows_each_layer() {
        let mut progress = Progress::default();
        progress.read(info("1.27", "Pulling from library/nginx", 0, 0));
        assert!(progress.layers.is_empty());
        progress.read(info("9c0abc9c5bd3", "Pulling fs layer", 0, 0));
        progress.read(info("1f2a3b4c5d6e", "Already exists", 0, 0));
        progress.read(info("9c0abc9c5bd3", "Downloading", 1_024, 4_096));
        let layer = &progress.layers[0];
        assert_eq!(layer.state, i32::from(ImageLayerState::Downloading));
        assert_eq!((layer.current_bytes, layer.total_bytes), (1_024, 4_096));
        progress.read(info("9c0abc9c5bd3", "Verifying Checksum", 0, 0));
        assert_eq!(progress.layers[0].current_bytes, 4_096);
        progress.read(info("9c0abc9c5bd3", "Extracting", 2_048, 4_096));
        assert_eq!(progress.layers[0].current_bytes, 2_048);
        progress.read(info("9c0abc9c5bd3", "Pull complete", 0, 0));
        progress.read(info("", "Digest: sha256:d2", 0, 0));
        assert_eq!(
            progress
                .layers
                .iter()
                .map(|layer| (layer.id.as_str(), layer.state, layer.current_bytes))
                .collect::<Vec<_>>(),
            [
                ("9c0abc9c5bd3", i32::from(ImageLayerState::Complete), 4_096),
                ("1f2a3b4c5d6e", i32::from(ImageLayerState::Exists), 0),
            ]
        );
        assert_eq!(progress.digest.as_deref(), Some("sha256:d2"));
        assert!(progress.updated());
        progress.read(info("", "Status: Image is up to date for nginx:1.27", 0, 0));
        assert!(!progress.updated());
    }

    #[test]
    fn registry_refusals_are_never_the_panels_own_session() {
        let stream = |message: &str| EngineError::DockerStreamError {
            error: message.into(),
        };
        let server = |status_code: u16, message: &str| EngineError::DockerResponseServerError {
            status_code,
            message: message.into(),
        };
        for (error, code) in [
            (
                server(
                    404,
                    "pull access denied for missing, repository does not exist",
                ),
                "NOT_FOUND",
            ),
            (
                server(
                    500,
                    "Head \"https://ghcr.io/v2/example/app/manifests/2.3\": unauthorized",
                ),
                "PERMISSION_DENIED",
            ),
            (
                stream("toomanyrequests: You have reached your pull rate limit"),
                "RESOURCE_EXHAUSTED",
            ),
            (
                stream("no matching manifest for linux/s390x in the manifest list entries"),
                "NOT_FOUND",
            ),
            (stream("read: connection reset by peer"), "UNAVAILABLE"),
        ] {
            assert_eq!(pull_failure(&error).code.as_str(), code, "{error}");
        }
    }
}
