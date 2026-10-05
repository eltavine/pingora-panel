use super::*;
use axum::http::HeaderMap;
use base64::Engine;
use futures_util::StreamExt;
use panel_application::{
    Backup, BackupContent, BackupDownload, BackupRequest, BackupState, BackupsPort, CommandContext,
    Operation, OperationLog, RequestScope, SitesRestored, ACTIVE_BUNDLE,
};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationCommand, ConfigurationOutput,
    ConfigurationPort, ConfigurationQuery, DraftInfo, LanguageChange, LanguageQuery, RevisionQuery,
};
use serde_json::{json, Value};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

const ID: &str = "6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4";
const ARCHIVE: &[u8] = b"an archive";

fn backup(state: BackupState) -> Backup {
    Backup {
        id: ID.to_owned(),
        contents: vec![BackupContent::Configuration, BackupContent::Sites],
        site_path: String::new(),
        state,
        requested_by: "ops".to_owned(),
        requested_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        finished_at: None,
        size_bytes: ARCHIVE.len() as u64,
        sha256: hex::encode(sha2::Sha256::digest(ARCHIVE)),
        files: 4,
        failure: None,
        product_version: "1.2.3".to_owned(),
    }
}

/// Backups that keep the requests they were asked for.
#[derive(Default)]
struct Backups {
    requested: Mutex<Vec<BackupRequest>>,
    /// The active revision a backup holds, as a bundle.
    active: Option<Value>,
}

#[async_trait]
impl BackupsPort for Backups {
    async fn list(&self, _: RequestScope) -> Result<Vec<Backup>> {
        Ok(vec![backup(BackupState::Completed)])
    }

    async fn get(&self, _: RequestScope, id: &str) -> Result<Backup> {
        if id == ID {
            Ok(backup(BackupState::Completed))
        } else {
            Err(PanelError::not_found(format!("backup {id} does not exist")))
        }
    }

    async fn create(&self, context: CommandContext, request: BackupRequest) -> Result<Backup> {
        assert_eq!(context.actor(), "ops");
        let mut created = backup(BackupState::Pending);
        created.contents = request.contents.clone();
        created.site_path = request.site_path.clone();
        self.requested.lock().unwrap().push(request);
        Ok(created)
    }

    async fn delete(&self, _: CommandContext, _: &str) -> Result<()> {
        Ok(())
    }

    async fn download(&self, _: RequestScope, _: &str) -> Result<BackupDownload> {
        Ok(BackupDownload {
            backup: backup(BackupState::Completed),
            chunks: futures_util::stream::iter([Ok(b"an ".to_vec()), Ok(b"archive".to_vec())])
                .boxed(),
        })
    }

    async fn member(&self, _: RequestScope, _: &str, path: &str) -> Result<Vec<u8>> {
        assert_eq!(path, ACTIVE_BUNDLE);
        self.active
            .as_ref()
            .map(|bundle| bundle.to_string().into_bytes())
            .ok_or_else(|| PanelError::not_found(format!("the archive holds no {path}")))
    }

    async fn restore_sites(&self, _: CommandContext, _: &str, path: &str) -> Result<SitesRestored> {
        assert_eq!(path, "shop");
        Ok(SitesRestored {
            files: 2,
            bytes: 30,
        })
    }
}

fn revision(id: u64, outcome: &str) -> Value {
    json!({
        "id": id, "draft_version": id, "language_version": 1, "content_hash": "ab",
        "author": "ops", "created_at": "2027-01-15T08:00:00Z", "outcome": outcome,
        "outcome_at": "2027-01-15T08:00:02Z",
    })
}

/// A draft, its revisions and the files they hold.
struct Draft(Mutex<(u64, BTreeMap<String, String>)>);

fn output(content: Value, version: u64) -> ConfigurationOutput {
    ConfigurationOutput {
        content: serde_json::to_vec(&content).unwrap(),
        etag: Some(format!("\"draft-{version}\"")),
        draft: DraftInfo {
            version,
            ..DraftInfo::default()
        },
    }
}

#[async_trait]
impl ConfigurationPort for Draft {
    async fn read(
        &self,
        _: RequestScope,
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        let draft = self.0.lock().unwrap();
        Ok(match query {
            ConfigurationQuery::Language(LanguageQuery::Source) => output(
                json!({ "language_version": 1, "version": draft.0, "files": draft.1 }),
                draft.0,
            ),
            ConfigurationQuery::Revision(RevisionQuery::Revisions { .. }) => output(
                json!({ "items": [revision(4, "failed"), revision(3, "active"), revision(2, "superseded")] }),
                draft.0,
            ),
            ConfigurationQuery::Revision(RevisionQuery::Revision { id: 3 }) => output(
                json!({ "revision": revision(3, "active"), "files": { "main.conf": "server live {}\n" } }),
                draft.0,
            ),
            _ => return Err(PanelError::unavailable("not read here")),
        })
    }

    async fn change(
        &self,
        _: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        let ConfigurationCommand::Language(LanguageChange::ReplaceSource { files }) =
            change.command
        else {
            return Err(PanelError::unavailable("only the files are replaced here"));
        };
        let mut draft = self.0.lock().unwrap();
        if change
            .if_match
            .is_some_and(|tag| tag != format!("\"draft-{}\"", draft.0))
        {
            return Err(PanelError::precondition_failed("the draft changed"));
        }
        *draft = (draft.0 + 1, files);
        Ok(output(json!({}), draft.0))
    }

    async fn apply(&self, _: CommandContext, _: ApplyRequest) -> Result<ApplyOutcome> {
        Err(PanelError::unavailable("nothing is applied here"))
    }
}

/// What the API recorded itself.
#[derive(Default)]
struct Recorded(Mutex<Vec<String>>);

#[async_trait]
impl OperationLog for Recorded {
    async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
        if let Operation::Backup {
            id,
            change: panel_application::BackupChange::ConfigurationRestored(result),
        } = operation
        {
            self.0.lock().unwrap().push(format!(
                "{id} {:?}",
                result.map_err(|error| error.code.as_str().to_owned())
            ));
        }
    }
}

struct Harness {
    app: axum::Router,
    backups: Arc<Backups>,
    draft: Arc<Draft>,
    recorded: Arc<Recorded>,
}

fn harness(active: Option<Value>) -> Harness {
    let backups = Arc::new(Backups {
        active,
        ..Backups::default()
    });
    let draft = Arc::new(Draft(Mutex::new((
        6,
        BTreeMap::from([("main.conf".to_owned(), "server draft {}\n".to_owned())]),
    ))));
    let recorded = Arc::new(Recorded::default());
    let app = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_backups(backups.clone())
        .with_configuration(draft.clone())
        .with_operation_log(recorded.clone()),
    );
    Harness {
        app,
        backups,
        draft,
        recorded,
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn change(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-actor", "ops")
        .header("idempotency-key", "backup-1")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn backups_holding_the_configuration_carry_its_bundles() {
    let harness = harness(None);
    let (status, headers, body) = send(
        &harness.app,
        change(
            "POST",
            "/api/v1/backups",
            &json!({ "contents": ["configuration", "sites"], "site_path": "/shop/" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{}", json_of(&body));
    assert_eq!(headers["location"], format!("/api/v1/backups/{ID}"));
    let created = json_of(&body);
    assert_eq!(created["state"], "pending");
    assert_eq!(created["contents"], json!(["configuration", "sites"]));
    assert_eq!(created["site_path"], "shop");

    {
        let requested = harness.backups.requested.lock().unwrap();
        let attachments = &requested[0].attachments;
        assert_eq!(
            attachments.keys().collect::<Vec<_>>(),
            ["configuration/active.json", "configuration/draft.json"]
        );
        let active: Value =
            serde_json::from_slice(&attachments["configuration/active.json"]).unwrap();
        assert_eq!(active["format"], "pingora-panel-configuration");
        assert_eq!(active["files"]["main.conf"], "server live {}\n");
        let draft: Value =
            serde_json::from_slice(&attachments["configuration/draft.json"]).unwrap();
        assert_eq!(draft["files"]["main.conf"], "server draft {}\n");
    }

    let (status, _, body) = send(
        &harness.app,
        change("POST", "/api/v1/backups", &json!({ "contents": ["sites"] })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{}", json_of(&body));
    assert!(
        harness.backups.requested.lock().unwrap()[1]
            .attachments
            .is_empty(),
        "only backups holding the configuration carry bundles"
    );

    let (status, _, body) = send(
        &harness.app,
        change(
            "POST",
            "/api/v1/backups",
            &json!({ "contents": ["everything"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", json_of(&body));
}

#[tokio::test]
async fn archives_are_downloaded_with_their_digest() {
    let harness = harness(None);
    let (status, _, body) = send(
        &harness.app,
        Request::get("/api/v1/backups").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = json_of(&body);
    assert_eq!(listed["backups"][0]["state"], "completed");
    assert_eq!(listed["backups"][0]["requested_at"], "2027-01-15T08:00:00Z");
    assert!(listed["backups"][0].get("site_path").is_none());

    let (status, headers, body) = send(
        &harness.app,
        Request::get(format!("/api/v1/backups/{ID}/archive"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, ARCHIVE);
    assert_eq!(headers["content-type"], "application/zstd");
    assert_eq!(headers["content-length"], ARCHIVE.len().to_string());
    assert_eq!(
        headers["content-disposition"],
        format!("attachment; filename=\"pingora-panel-backup-{ID}.tar.zst\"")
    );
    let digest = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(ARCHIVE));
    assert_eq!(headers["repr-digest"], format!("sha-256=:{digest}:"));
    assert_eq!(headers["cache-control"], "no-store");

    let (status, _, _) = send(
        &harness.app,
        Request::get("/api/v1/backups/b-2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(
        &harness.app,
        change("DELETE", &format!("/api/v1/backups/{ID}"), &json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_sites_and_the_configuration_are_restored_from_a_backup() {
    let active = json!({
        "format": "pingora-panel-configuration", "language_version": 1,
        "files": { "main.conf": "server live {}\n" },
    });
    let harness = harness(Some(active));
    let restores = format!("/api/v1/backups/{ID}/restores");

    let (status, _, body) = send(
        &harness.app,
        change(
            "POST",
            &restores,
            &json!({ "target": "sites", "site_path": "shop/" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", json_of(&body));
    assert_eq!(
        json_of(&body),
        json!({ "target": "sites", "site_path": "shop", "files": 2, "bytes": 30 })
    );

    let mut stale = change("POST", &restores, &json!({ "target": "configuration" }));
    stale
        .headers_mut()
        .insert("if-match", "\"draft-5\"".parse().unwrap());
    let (status, _, _) = send(&harness.app, stale).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);

    let (status, _, body) = send(
        &harness.app,
        change("POST", &restores, &json!({ "target": "configuration" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", json_of(&body));
    assert_eq!(
        json_of(&body),
        json!({ "target": "configuration", "draft_version": 7 })
    );
    assert_eq!(
        harness.draft.0.lock().unwrap().1["main.conf"],
        "server live {}\n"
    );
    assert_eq!(
        *harness.recorded.0.lock().unwrap(),
        [
            format!("{ID} Err(\"PRECONDITION_FAILED\")"),
            format!("{ID} Ok(7)")
        ]
    );

    let without = self::harness(None);
    let (status, _, body) = send(
        &without.app,
        change("POST", &restores, &json!({ "target": "configuration" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", json_of(&body));
    assert!(json_of(&body)["detail"]
        .as_str()
        .unwrap()
        .contains("no active configuration"));
}
