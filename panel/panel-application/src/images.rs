//! The images on the container engines `ops-agent` reaches (ADR 0031).

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

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
}

/// An images port that records each removal, refused or not.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
    use std::sync::Mutex;

    /// Removes `redis:7` and refuses everything else.
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
        }
    }

    #[tokio::test]
    async fn every_removal_is_recorded_refused_or_not() {
        let recorder = Arc::new(Recorder::default());
        let images = RecordedImages::new(Arc::new(Engine), recorder.clone());
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap();
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
}
