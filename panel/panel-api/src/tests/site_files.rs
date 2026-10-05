use super::*;
use axum::http::HeaderMap;
use panel_application::{
    CommandContext, RequestScope, SiteDirectory, SiteEntry, SiteEntryKind, SiteFile,
    SiteFileWritten, SiteFilesPort, SitePath, SiteRemoval, WriteCondition,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

/// Files by path, each with its content.
#[derive(Default)]
struct Files(Mutex<BTreeMap<String, Vec<u8>>>);

fn tag(content: &[u8]) -> String {
    format!("\"{}\"", content.len())
}

#[async_trait]
impl SiteFilesPort for Files {
    async fn directory(&self, _: RequestScope, path: SitePath) -> Result<SiteDirectory> {
        if path.as_str() != "shop" {
            return Err(PanelError::not_found(format!("{path} does not exist")));
        }
        Ok(SiteDirectory {
            path,
            entries: vec![
                SiteEntry {
                    name: "assets".into(),
                    kind: SiteEntryKind::Directory,
                    size_bytes: 0,
                    modified: None,
                },
                SiteEntry {
                    name: "index.html".into(),
                    kind: SiteEntryKind::File,
                    size_bytes: 13,
                    modified: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                },
            ],
        })
    }

    async fn file(&self, _: RequestScope, path: SitePath) -> Result<SiteFile> {
        let content = self
            .0
            .lock()
            .unwrap()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| PanelError::not_found(format!("{path} does not exist")))?;
        Ok(SiteFile {
            tag: tag(&content),
            path,
            content,
            modified: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        })
    }

    async fn write_file(
        &self,
        context: CommandContext,
        path: SitePath,
        content: Vec<u8>,
        condition: WriteCondition,
    ) -> Result<SiteFileWritten> {
        assert_eq!(context.actor(), "ops");
        let mut files = self.0.lock().unwrap();
        let before = files.get(path.as_str()).map(|content| tag(content));
        match (&before, &condition) {
            (Some(_), WriteCondition::Absent) => {
                return Err(PanelError::precondition_failed("it already exists"))
            }
            (Some(current), WriteCondition::Tagged(expected)) if current != expected => {
                return Err(PanelError::precondition_failed("it changed"))
            }
            _ => {}
        }
        let written = SiteFileWritten {
            path: path.clone(),
            size_bytes: content.len() as u64,
            sha256: "ab".repeat(32),
            tag: tag(&content),
            created: before.is_none(),
        };
        files.insert(path.as_str().to_owned(), content);
        Ok(written)
    }

    async fn create_directory(&self, _: CommandContext, _: SitePath) -> Result<()> {
        Ok(())
    }

    async fn remove(
        &self,
        _: CommandContext,
        path: SitePath,
        recursive: bool,
    ) -> Result<SiteRemoval> {
        if !recursive {
            return Err(PanelError::conflict(format!("{path} is not empty")));
        }
        Ok(SiteRemoval {
            path,
            kind: SiteEntryKind::Directory,
            removed: 3,
        })
    }
}

fn app(files: Option<Arc<Files>>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(match files {
        Some(files) => state.with_site_files(files),
        None => state,
    })
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn change(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor", "ops")
        .header("idempotency-key", "file-1")
        .header("x-deadline", "2099-01-01T00:00:00Z")
}

#[tokio::test]
async fn directories_are_listed_and_files_read_as_attachments() {
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .unwrap()
        .insert("shop/index.html".into(), b"<h1>Shop</h1>".to_vec());
    let app = app(Some(files));
    let (status, _, body) = send(
        &app,
        Request::get("/api/v1/site-files?path=/shop/")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = json(&body);
    assert_eq!(listed["path"], "shop");
    assert_eq!(listed["entries"][0]["kind"], "directory");
    assert_eq!(listed["entries"][1]["modified"], "2027-01-15T08:00:00Z");

    let (status, headers, body) = send(
        &app,
        Request::get("/api/v1/site-files/content?path=shop/index.html")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"<h1>Shop</h1>");
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=\"index.html\""
    );
    assert_eq!(headers["etag"], "\"13\"");

    for path in ["../etc/passwd", "shop/../../x", "shop%5C..%5Cx"] {
        let (status, _, _) = send(
            &app,
            Request::get(format!("/api/v1/site-files/content?path={path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
}

#[tokio::test]
async fn files_are_written_on_their_conditions() {
    let files = Arc::new(Files::default());
    let app = app(Some(files.clone()));
    let put = |condition: Option<(&str, &str)>, body: &'static str| {
        let mut request = change("PUT", "/api/v1/site-files/content?path=shop/index.html")
            .header("content-type", "text/html");
        if let Some((name, value)) = condition {
            request = request.header(name, value);
        }
        request.body(Body::from(body)).unwrap()
    };
    let (status, headers, body) =
        send(&app, put(Some(("if-none-match", "*")), "<h1>Shop</h1>")).await;
    assert_eq!(status, StatusCode::CREATED, "{}", json(&body));
    assert_eq!(headers["etag"], "\"13\"");
    assert_eq!(json(&body)["created"], true);
    let (status, _, _) = send(&app, put(Some(("if-none-match", "*")), "<h1>Again</h1>")).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    let (status, _, body) = send(&app, put(Some(("if-match", "\"13\"")), "<h1>New</h1>")).await;
    assert_eq!(status, StatusCode::OK, "{}", json(&body));
    let (status, _, _) = send(&app, put(Some(("if-match", "\"13\"")), "<h1>Stale</h1>")).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    let (status, _, _) = send(&app, put(Some(("if-none-match", "\"12\"")), "x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        files.0.lock().unwrap()["shop/index.html"],
        b"<h1>New</h1>".to_vec()
    );
}

#[tokio::test]
async fn directories_are_created_and_entries_removed() {
    let app = app(Some(Arc::new(Files::default())));
    let (status, _, _) = send(
        &app,
        change("POST", "/api/v1/site-files/directories?path=blog/2027")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(
        &app,
        change("DELETE", "/api/v1/site-files?path=shop")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, body) = send(
        &app,
        change("DELETE", "/api/v1/site-files?path=shop&recursive=true")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["removed"], 3);
}

#[tokio::test]
async fn without_the_directory_the_sites_files_are_unsupported() {
    let (status, _, body) = send(
        &app(None),
        Request::get("/api/v1/site-files")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json(&body)["code"], "UNSUPPORTED_CAPABILITY");
}
