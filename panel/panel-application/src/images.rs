//! The images on the container engines `ops-agent` reaches (ADR 0031).

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, fmt, pin::Pin, sync::Arc, time::SystemTime};
use zeroize::Zeroizing;

/// An image as a list shows it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Image {
    /// Such as `sha256:4f1c…`.
    pub id: String,
    /// Such as `nginx:1.27`; none for an image nothing names any more.
    pub tags: Vec<String>,
    pub digests: Vec<String>,
    pub created: Option<SystemTime>,
    /// Layers it shares with other images included.
    pub size_bytes: u64,
    /// The containers, running or not, created from it.
    pub containers: u32,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageList {
    pub observed_at: Option<SystemTime>,
    /// By tag; images nothing names last.
    pub images: Vec<Image>,
}

/// An image's configuration, without its environment or command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageDetail {
    pub image: Image,
    pub architecture: Option<String>,
    pub variant: Option<String>,
    pub os: Option<String>,
    pub author: Option<String>,
    pub comment: Option<String>,
    pub user: Option<String>,
    pub working_directory: Option<String>,
    /// Such as `80/tcp`.
    pub exposed_ports: Vec<String>,
    pub volumes: Vec<String>,
    pub stop_signal: Option<String>,
    pub layers: u32,
}

/// What removing a reference to an image did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageRemoval {
    /// The image's ID before it was removed.
    pub id: String,
    /// The references removed.
    pub untagged: Vec<String>,
    /// The images and layers deleted; none when other references remain.
    pub deleted: Vec<String>,
}

/// For a registry that wants a sign-in: used for one pull and never kept.
#[derive(Clone)]
pub struct RegistryCredentials {
    pub username: String,
    /// A password or an access token.
    pub password: Zeroizing<String>,
}

impl fmt::Debug for RegistryCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryCredentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct ImagePullRequest {
    /// Such as `nginx:1.27` or `ghcr.io/example/app@sha256:…`; a name alone
    /// is its `latest` tag.
    pub reference: String,
    /// Such as `linux/arm64`; the engine's own when `None`.
    pub platform: Option<String>,
    pub credentials: Option<RegistryCredentials>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ImageLayerState {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageLayerProgress {
    /// Such as `a2abf6c4d29d`.
    pub id: String,
    pub state: ImageLayerState,
    /// How far downloading or extracting it got, of how much; 0 while
    /// unknown.
    pub current_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImagePulled {
    pub image: Image,
    /// Such as `sha256:…`: what the registry served.
    pub digest: Option<String>,
    /// Whether the engine downloaded a newer image than it had.
    pub updated: bool,
}

/// What a pull says as it goes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImagePullEvent {
    /// How far each layer got, in the order the engine first named them.
    Progress(Vec<ImageLayerProgress>),
    /// What was pulled; the last event.
    Pulled(ImagePulled),
}

/// A pull as it goes. It ends after [`ImagePullEvent::Pulled`] or an error
/// saying why not; dropping it cancels the pull.
pub type ImagePull = Pin<Box<dyn Stream<Item = Result<ImagePullEvent>> + Send>>;

#[async_trait]
pub trait ImagesPort: Send + Sync {
    /// An engine's images, searched by tag or ID.
    async fn images(
        &self,
        scope: RequestScope,
        engine: String,
        search: String,
    ) -> Result<ImageList>;

    /// An image named by its ID, a prefix of its ID, or a reference.
    async fn inspect_image(
        &self,
        scope: RequestScope,
        engine: String,
        image: String,
    ) -> Result<ImageDetail>;

    /// Removes a reference to an image, and the image once nothing names
    /// it; `force` also removes one stopped containers use.
    async fn remove_image(
        &self,
        context: CommandContext,
        engine: String,
        image: String,
        force: bool,
    ) -> Result<ImageRemoval>;

    /// Pulls an image from its registry.
    async fn pull_image(
        &self,
        context: CommandContext,
        engine: String,
        request: ImagePullRequest,
    ) -> Result<ImagePull>;
}

/// The port of an installation whose agent manages no engine.
pub struct NoImages;

impl NoImages {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("no host agent manages container engines here")
    }
}

#[async_trait]
impl ImagesPort for NoImages {
    async fn images(&self, _: RequestScope, _: String, _: String) -> Result<ImageList> {
        Err(Self::refusal())
    }

    async fn inspect_image(&self, _: RequestScope, _: String, _: String) -> Result<ImageDetail> {
        Err(Self::refusal())
    }

    async fn remove_image(
        &self,
        _: CommandContext,
        _: String,
        _: String,
        _: bool,
    ) -> Result<ImageRemoval> {
        Err(Self::refusal())
    }

    async fn pull_image(
        &self,
        _: CommandContext,
        _: String,
        _: ImagePullRequest,
    ) -> Result<ImagePull> {
        Err(Self::refusal())
    }
}

/// An images port that records each removal and pull, refused or not; a
/// pull is recorded once it ends.
pub struct RecordedImages {
    inner: Arc<dyn ImagesPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedImages {
    pub fn new(inner: Arc<dyn ImagesPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl ImagesPort for RecordedImages {
    async fn images(
        &self,
        scope: RequestScope,
        engine: String,
        search: String,
    ) -> Result<ImageList> {
        self.inner.images(scope, engine, search).await
    }

    async fn inspect_image(
        &self,
        scope: RequestScope,
        engine: String,
        image: String,
    ) -> Result<ImageDetail> {
        self.inner.inspect_image(scope, engine, image).await
    }

    async fn remove_image(
        &self,
        context: CommandContext,
        engine: String,
        image: String,
        force: bool,
    ) -> Result<ImageRemoval> {
        let result = self
            .inner
            .remove_image(context.clone(), engine.clone(), image.clone(), force)
            .await;
        let operation = Operation::ImageRemoval {
            engine: &engine,
            image: &image,
            force,
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn pull_image(
        &self,
        context: CommandContext,
        engine: String,
        request: ImagePullRequest,
    ) -> Result<ImagePull> {
        let reference = request.reference.clone();
        let pull = match self
            .inner
            .pull_image(context.clone(), engine.clone(), request)
            .await
        {
            Ok(pull) => pull,
            Err(error) => {
                let operation = Operation::ImagePull {
                    engine: &engine,
                    reference: &reference,
                    result: Err(&error),
                };
                self.log.record(&context, operation).await;
                return Err(error);
            }
        };
        let log = Arc::clone(&self.log);
        let recorded = futures_util::stream::unfold(Some(pull), move |pull| {
            let (log, context, engine, reference) = (
                log.clone(),
                context.clone(),
                engine.clone(),
                reference.clone(),
            );
            async move {
                let mut pull = pull?;
                let event = match pull.next().await {
                    Some(Ok(ImagePullEvent::Progress(layers))) => {
                        return Some((Ok(ImagePullEvent::Progress(layers)), Some(pull)));
                    }
                    Some(event) => event,
                    None => Err(PanelError::unavailable(
                        "the host agent stopped the pull without saying why",
                    )),
                };
                let result = match &event {
                    Ok(ImagePullEvent::Pulled(pulled)) => Ok(pulled),
                    Ok(ImagePullEvent::Progress(_)) => return Some((event, Some(pull))),
                    Err(error) => Err(error),
                };
                let operation = Operation::ImagePull {
                    engine: &engine,
                    reference: &reference,
                    result,
                };
                log.record(&context, operation).await;
                Some((event, None))
            }
        });
        Ok(Box::pin(recorded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use std::sync::Mutex;

    /// Removes `redis:7` and refuses everything else; pulls busybox, refuses
    /// missing, and breaks off pulling flaky and cut, cut without a word.
    struct Engine;

    #[async_trait]
    impl ImagesPort for Engine {
        async fn images(&self, _: RequestScope, _: String, _: String) -> Result<ImageList> {
            Err(NoImages::refusal())
        }

        async fn inspect_image(
            &self,
            _: RequestScope,
            _: String,
            _: String,
        ) -> Result<ImageDetail> {
            Err(NoImages::refusal())
        }

        async fn remove_image(
            &self,
            _: CommandContext,
            _: String,
            image: String,
            _: bool,
        ) -> Result<ImageRemoval> {
            if image != "redis:7" {
                return Err(PanelError::conflict(format!("{image} is in use")));
            }
            Ok(ImageRemoval {
                id: "sha256:bb".into(),
                untagged: vec![image],
                deleted: vec!["sha256:bb".into()],
            })
        }

        async fn pull_image(
            &self,
            _: CommandContext,
            _: String,
            request: ImagePullRequest,
        ) -> Result<ImagePull> {
            let progress = Ok(ImagePullEvent::Progress(vec![ImageLayerProgress {
                id: "9c0abc9c5bd3".into(),
                state: ImageLayerState::Downloading,
                current_bytes: 1_024,
                total_bytes: 4_096,
            }]));
            let events = match request.reference.as_str() {
                "busybox:1.37" => vec![
                    progress,
                    Ok(ImagePullEvent::Pulled(ImagePulled {
                        image: Image {
                            id: "sha256:dd".into(),
                            ..Image::default()
                        },
                        digest: Some("sha256:d2".into()),
                        updated: true,
                    })),
                ],
                "flaky:1" => vec![progress, Err(PanelError::unavailable("connection reset"))],
                "cut:1" => vec![progress],
                reference => return Err(PanelError::not_found(format!("no {reference}"))),
            };
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::ImageRemoval {
                engine,
                image,
                force,
                result,
            } = operation
            {
                let outcome = match result {
                    Ok(removal) => removal.deleted.join(","),
                    Err(error) => error.code.as_str().to_owned(),
                };
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("{engine}/{image} force={force} {outcome}"));
            }
            if let Operation::ImagePull {
                engine,
                reference,
                result,
            } = operation
            {
                let outcome = match result {
                    Ok(pulled) => pulled.image.id.clone(),
                    Err(error) => error.code.as_str().to_owned(),
                };
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("pull {engine}/{reference} {outcome}"));
            }
        }
    }

    fn context() -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn every_removal_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let images = RecordedImages::new(Arc::new(Engine), recorder.clone());
        let context = context();
        images
            .remove_image(context.clone(), "docker".into(), "redis:7".into(), true)
            .await
            .unwrap();
        images
            .remove_image(context, "docker".into(), "nginx:1.27".into(), false)
            .await
            .unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                "docker/redis:7 force=true sha256:bb",
                "docker/nginx:1.27 force=false CONFLICT"
            ]
        );
    }

    #[tokio::test]
    async fn every_pull_is_recorded_once_it_ends() {
        let recorder = Arc::new(Recorder::default());
        let images = RecordedImages::new(Arc::new(Engine), recorder.clone());
        let pull = |reference: &str| {
            images.pull_image(
                context(),
                "docker".into(),
                ImagePullRequest {
                    reference: reference.into(),
                    platform: None,
                    credentials: Some(RegistryCredentials {
                        username: "ci".into(),
                        password: Zeroizing::new("hunter2".into()),
                    }),
                },
            )
        };
        let events: Vec<_> = pull("busybox:1.37").await.unwrap().collect().await;
        assert!(matches!(events[0], Ok(ImagePullEvent::Progress(_))));
        assert!(matches!(events[1], Ok(ImagePullEvent::Pulled(_))));
        assert_eq!(events.len(), 2);
        assert_eq!(
            pull("missing:1").await.err().unwrap().code.as_str(),
            "NOT_FOUND"
        );
        let broken: Vec<_> = pull("flaky:1").await.unwrap().collect().await;
        assert!(broken[1].is_err());
        let cut: Vec<_> = pull("cut:1").await.unwrap().collect().await;
        assert_eq!(cut.len(), 2, "an end without a word is an error");
        assert_eq!(cut[1].as_ref().unwrap_err().code.as_str(), "UNAVAILABLE");
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                "pull docker/busybox:1.37 sha256:dd",
                "pull docker/missing:1 NOT_FOUND",
                "pull docker/flaky:1 UNAVAILABLE",
                "pull docker/cut:1 UNAVAILABLE"
            ]
        );
    }

    #[test]
    fn registry_passwords_never_print() {
        let credentials = RegistryCredentials {
            username: "ci".into(),
            password: Zeroizing::new("hunter2".into()),
        };
        let printed = format!("{credentials:?}");
        assert!(
            printed.contains("ci") && !printed.contains("hunter2"),
            "{printed}"
        );
    }
}
