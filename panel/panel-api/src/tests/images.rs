use super::*;
use panel_application::{
    CommandContext, Image, ImageDetail, ImageList, ImageRemoval, ImagesPort, RequestScope,
};
use serde_json::Value;
use std::{
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

/// nginx, which a running container uses, and redis, which goes; records
/// the images it was asked about.
#[derive(Default)]
struct Images(Mutex<Vec<String>>);

fn nginx() -> Image {
    Image {
        id: "sha256:aa".into(),
        tags: vec!["nginx:1.27".into()],
        digests: vec!["nginx@sha256:d1".into()],
        created: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        size_bytes: 50_000_000,
        containers: 1,
        labels: [("maintainer".to_owned(), "NGINX".to_owned())].into(),
    }
}

#[async_trait]
impl ImagesPort for Images {
    async fn images(&self, _: RequestScope, _: String, search: String) -> Result<ImageList> {
        self.0.lock().unwrap().push(format!("list {search}"));
        Ok(ImageList {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_010)),
            images: vec![nginx()],
        })
    }

    async fn inspect_image(
        &self,
        _: RequestScope,
        _: String,
        image: String,
    ) -> Result<ImageDetail> {
        self.0.lock().unwrap().push(format!("inspect {image}"));
        if image == "ghost:1" {
            return Err(PanelError::not_found("No such image: ghost:1"));
        }
        Ok(ImageDetail {
            image: nginx(),
            architecture: Some("arm64".into()),
            variant: Some("v8".into()),
            os: Some("linux".into()),
            author: None,
            comment: None,
            user: Some("nginx".into()),
            working_directory: None,
            exposed_ports: vec!["80/tcp".into()],
            volumes: Vec::new(),
            stop_signal: Some("SIGQUIT".into()),
            layers: 7,
        })
    }

    async fn remove_image(
        &self,
        _: CommandContext,
        _: String,
        image: String,
        force: bool,
    ) -> Result<ImageRemoval> {
        self.0
            .lock()
            .unwrap()
            .push(format!("remove {image} force={force}"));
        if image != "redis:7" {
            return Err(PanelError::conflict(
                "image is being used by running container b2",
            ));
        }
        Ok(ImageRemoval {
            id: "sha256:bb".into(),
            untagged: vec![image],
            deleted: vec!["sha256:bb".into()],
        })
    }
}

fn app(images: Option<Arc<Images>>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(match images {
        Some(images) => state.with_images(images),
        None => state,
    })
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(app, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn delete(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(
        app,
        Request::delete(path)
            .header("x-actor", "ops")
            .header("idempotency-key", "image-1")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn without_the_agent_images_are_unsupported() {
    let (status, problem) = get(&app(None), "/api/v1/container-engines/docker/images").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "UNSUPPORTED_CAPABILITY");
}

#[tokio::test]
async fn images_are_listed_and_inspected_by_any_reference() {
    let images = Arc::new(Images::default());
    let app = app(Some(Arc::clone(&images)));
    let (status, list) = get(&app, "/api/v1/container-engines/docker/images?search=nginx").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["observed_at"], "2027-01-15T08:00:10Z");
    let nginx = &list["images"][0];
    assert_eq!(nginx["tags"][0], "nginx:1.27");
    assert_eq!(nginx["created"], "2027-01-15T08:00:00Z");
    assert_eq!(
        (nginx["containers"].as_u64(), nginx["size_bytes"].as_u64()),
        (Some(1), Some(50_000_000))
    );

    let path = "/api/v1/container-engines/docker/images";
    let (status, detail) = get(&app, &format!("{path}/ghcr.io%2Fexample%2Fapp:2.3")).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["architecture"], "arm64");
    assert_eq!(detail["author"], Value::Null);
    assert_eq!(detail["exposed_ports"][0], "80/tcp");
    let (status, _) = get(&app, &format!("{path}/ghost:1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for escaping in ["..%2Fcontainers%2Fb2", "nginx%2F..%2F..", "%2Fnginx"] {
        let (status, _) = get(&app, &format!("{path}/{escaping}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{escaping}");
    }
    assert_eq!(
        *images.0.lock().unwrap(),
        [
            "list nginx",
            "inspect ghcr.io/example/app:2.3",
            "inspect ghost:1"
        ]
    );
}

#[tokio::test]
async fn images_are_removed_or_refused() {
    let images = Arc::new(Images::default());
    let app = app(Some(Arc::clone(&images)));
    let path = "/api/v1/container-engines/docker/images";
    let (status, removed) = delete(&app, &format!("{path}/redis:7?force=true")).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["untagged"][0], "redis:7");
    assert_eq!(removed["deleted"][0], "sha256:bb");
    let (status, problem) = delete(&app, &format!("{path}/nginx:1.27")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(
        *images.0.lock().unwrap(),
        ["remove redis:7 force=true", "remove nginx:1.27 force=false"]
    );
}
