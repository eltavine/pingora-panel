use super::*;
use panel_application::{
    CommandContext, Image, ImageDetail, ImageLayerProgress, ImageLayerState, ImageList, ImagePull,
    ImagePullEvent, ImagePullRequest, ImagePulled, ImageRemoval, ImagesPort, RequestScope,
};
use serde_json::Value;
use std::{
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

/// nginx, which a running container uses, and redis, which goes; busybox,
/// which pulls, and flaky, which breaks off. Records the images it was
/// asked about.
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

    async fn pull_image(
        &self,
        _: CommandContext,
        engine: String,
        request: ImagePullRequest,
    ) -> Result<ImagePull> {
        let credentials = request.credentials.as_ref();
        self.0.lock().unwrap().push(format!(
            "pull {engine} {} platform={} user={} password={}",
            request.reference,
            request.platform.as_deref().unwrap_or("-"),
            credentials.map_or("-", |credentials| credentials.username.as_str()),
            credentials.is_some_and(|credentials| credentials.password.as_str() == "hunter2"),
        ));
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
                        tags: vec!["busybox:1.37".into()],
                        ..nginx()
                    },
                    digest: Some("sha256:d2".into()),
                    updated: true,
                })),
            ],
            "flaky:1" => vec![progress, Err(PanelError::unavailable("connection reset"))],
            reference => vec![Err(PanelError::not_found(format!("no {reference}")))],
        };
        Ok(Box::pin(futures_util::stream::iter(events)))
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

async fn pull(app: &axum::Router, body: Value) -> (StatusCode, Option<String>, Vec<Value>) {
    let request = Request::post("/api/v1/container-engines/docker/image-pulls")
        .header("content-type", "application/json")
        .header("x-actor", "ops")
        .header("idempotency-key", "pull-1")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get("content-type")
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let messages = if kind.as_deref() == Some("text/event-stream") {
        text.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect()
    } else {
        vec![serde_json::from_str(&text).unwrap_or(Value::Null)]
    };
    (status, kind, messages)
}

#[tokio::test]
async fn images_are_pulled_as_server_sent_events() {
    let images = Arc::new(Images::default());
    let app = app(Some(images.clone()));
    let (status, kind, messages) = pull(
        &app,
        serde_json::json!({
            "reference": " busybox:1.37 ",
            "platform": "linux/arm64",
            "credentials": {"username": "ci", "password": "hunter2"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{messages:?}");
    assert_eq!(kind.as_deref(), Some("text/event-stream"));
    assert_eq!(messages[0]["kind"], "progress");
    assert_eq!(messages[0]["layers"][0]["state"], "downloading");
    assert_eq!(messages[0]["layers"][0]["total_bytes"], 4_096);
    assert_eq!(messages[1]["kind"], "pulled");
    assert_eq!(messages[1]["image"]["tags"][0], "busybox:1.37");
    assert_eq!(messages[1]["digest"], "sha256:d2");
    assert_eq!(messages[1]["updated"], true);
    assert_eq!(messages.len(), 2);

    let (status, _, problem) = pull(&app, serde_json::json!({"reference": "missing:1"})).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a refusal before anything is pulled"
    );
    assert_eq!(problem[0]["code"], "NOT_FOUND");
    let (status, _, messages) = pull(&app, serde_json::json!({"reference": "flaky:1"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages[1]["kind"], "failed");
    assert_eq!(messages[1]["error"]["code"], "UNAVAILABLE");

    for body in [
        serde_json::json!({"reference": "../etc"}),
        serde_json::json!({"reference": ""}),
        serde_json::json!({"reference": "nginx", "platform": "x".repeat(65)}),
        serde_json::json!({"reference": "nginx", "all_tags": true}),
    ] {
        let (status, _, _) = pull(&app, body.clone()).await;
        assert!(status.is_client_error(), "{body} {status}");
    }
    assert_eq!(
        *images.0.lock().unwrap(),
        [
            "pull docker busybox:1.37 platform=linux/arm64 user=ci password=true",
            "pull docker missing:1 platform=- user=- password=false",
            "pull docker flaky:1 platform=- user=- password=false"
        ]
    );
}
