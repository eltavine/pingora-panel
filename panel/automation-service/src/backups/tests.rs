use super::*;
use crate::{SqliteJobStore, MIGRATIONS};
use panel_context::{IdempotencyKey as Key, RequestDeadline, RequestId};
use panel_platform::ServiceName;
use panel_sqlite::testing::TestDatabase;

fn context() -> CommandContext {
    CommandContext::new(
        RequestId::new("request-1").unwrap(),
        RequestId::new("request-1").unwrap(),
        "alice",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        Key::new("key-1").unwrap(),
    )
    .unwrap()
}

/// A configuration database with one site, as its module keeps it.
async fn configuration(directory: &Path) {
    let options = SqliteConnectOptions::new()
        .filename(directory.join("config.db"))
        .create_if_missing(true);
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("CREATE TABLE sites (name TEXT PRIMARY KEY)")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sites VALUES ('shop')")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

fn sites() -> tempfile::TempDir {
    let sites = tempfile::tempdir().unwrap();
    let root = sites.path();
    fs::create_dir_all(root.join("shop/css")).unwrap();
    fs::write(root.join("shop/index.html"), "<h1>Shop</h1>\n").unwrap();
    fs::write(root.join("shop/css/site.css"), "body {}\n").unwrap();
    fs::write(root.join("shop/.index.html.17.writing"), "half").unwrap();
    fs::create_dir_all(root.join("blog")).unwrap();
    fs::write(root.join("blog/index.html"), "<h1>Blog</h1>\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", root.join("shop/passwd")).unwrap();
    sites
}

async fn backups(database: &TestDatabase, sites: Option<&Path>, kept: usize) -> Backups {
    let jobs = Arc::new(SqliteJobStore::new(
        database.database(),
        ServiceName::new("automation-service").unwrap(),
    ));
    Backups::new(
        database.database().clone(),
        jobs,
        database.directory(),
        sites.map(Path::to_owned),
        kept,
        "1.2.3",
    )
}

async fn queued(database: &TestDatabase) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = ?1")
        .bind(TAKE_JOB)
        .fetch_one(database.database().pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn backups_are_taken_in_the_background_and_hold_what_was_asked() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    configuration(database.directory()).await;
    let sites = sites();
    let backups = backups(&database, Some(sites.path()), 10).await;

    let draft = br#"{"format":"pingora-panel-configuration","files":{}}"#.to_vec();
    let requested = backups
        .create(
            &context(),
            BackupRequest {
                contents: vec![BackupContent::Sites, BackupContent::Configuration],
                site_path: String::new(),
                attachments: BTreeMap::from([(
                    "configuration/draft.json".to_owned(),
                    draft.clone(),
                )]),
            },
        )
        .await
        .unwrap();
    assert_eq!(requested.state, BackupState::Pending);
    assert_eq!(
        requested.contents,
        [BackupContent::Configuration, BackupContent::Sites]
    );
    assert_eq!(requested.requested_by, "alice");
    assert_eq!(queued(&database).await, 1);

    backups.take(requested.id).await.unwrap();
    let taken = backups.get(&requested.id.to_string()).await.unwrap();
    assert_eq!(taken.state, BackupState::Completed, "{:?}", taken.failure);
    assert_eq!(
        taken.files, 5,
        "the database, the draft and three site files"
    );
    assert_eq!(taken.sha256.len(), 64);
    assert!(taken.finished_at.is_some());

    let (_, archive) = backups.archive(&taken.id.to_string()).await.unwrap();
    assert_eq!(fs::metadata(&archive).unwrap().len(), taken.size_bytes);
    let manifest = panel_backup::verify(&archive).unwrap();
    assert_eq!(manifest.contents, ["configuration", "sites"]);
    assert_eq!(manifest.product_version, "1.2.3");
    let paths: Vec<&str> = manifest
        .members
        .iter()
        .map(|member| member.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "configuration/draft.json",
            "databases/config.db",
            "sites/blog/index.html",
            "sites/shop/css/site.css",
            "sites/shop/index.html",
        ],
        "links and files being written are left out"
    );
    assert_eq!(
        backups
            .member(&taken.id.to_string(), "configuration/draft.json")
            .await
            .unwrap(),
        draft
    );

    let copy = database.directory().join("restored-config.db");
    fs::write(
        &copy,
        backups
            .member(&taken.id.to_string(), "databases/config.db")
            .await
            .unwrap(),
    )
    .unwrap();
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&copy))
            .await
            .unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM sites")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(name, "shop", "the copy is a database of its own");
    connection.close().await.unwrap();

    assert!(
        !fs::read_dir(database.directory().join("backups"))
            .unwrap()
            .any(|entry| working(&entry.unwrap().file_name())),
        "nothing is left from taking it"
    );
}

#[tokio::test]
async fn a_site_directory_is_restored_in_place_and_nothing_else() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    let sites = sites();
    let backups = backups(&database, Some(sites.path()), 10).await;
    let taken = backups
        .create(
            &context(),
            BackupRequest {
                contents: vec![BackupContent::Sites],
                site_path: "shop".to_owned(),
                ..BackupRequest::default()
            },
        )
        .await
        .unwrap();
    backups.take(taken.id).await.unwrap();
    let id = taken.id.to_string();

    let root = sites.path();
    fs::write(root.join("shop/index.html"), "<h1>Changed</h1>\n").unwrap();
    fs::write(root.join("shop/new.html"), "new").unwrap();
    fs::write(root.join("blog/index.html"), "<h1>Blog, changed</h1>\n").unwrap();

    let restored = backups.restore_sites(&id, "shop").await.unwrap();
    assert_eq!(restored.files, 2);
    assert_eq!(
        fs::read_to_string(root.join("shop/index.html")).unwrap(),
        "<h1>Shop</h1>\n"
    );
    assert!(!root.join("shop/new.html").exists());
    assert_eq!(
        fs::read_to_string(root.join("blog/index.html")).unwrap(),
        "<h1>Blog, changed</h1>\n",
        "only the directory restored changes"
    );
    assert!(!fs::read_dir(root)
        .unwrap()
        .any(|entry| working(&entry.unwrap().file_name())));

    fs::remove_dir_all(root.join("shop")).unwrap();
    backups.restore_sites(&id, "shop").await.unwrap();
    assert!(
        root.join("shop/css/site.css").is_file(),
        "a removed directory comes back"
    );

    for (path, code) in [
        ("blog", "NOT_FOUND"),
        ("", "INVALID_ARGUMENT"),
        ("../etc", "INVALID_ARGUMENT"),
        (".pingora-panel-x", "INVALID_ARGUMENT"),
    ] {
        let refused = backups.restore_sites(&id, path).await.unwrap_err();
        assert_eq!(refused.code.as_str(), code, "{path}: {refused}");
    }
}

#[tokio::test]
async fn failures_are_kept_with_the_backup_and_old_backups_are_removed() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    configuration(database.directory()).await;
    let backups = backups(&database, None, 2).await;
    let take = |contents: Vec<BackupContent>| {
        let backups = backups.clone();
        async move {
            let backup = backups
                .create(
                    &context(),
                    BackupRequest {
                        contents,
                        ..BackupRequest::default()
                    },
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            backups.take(backup.id).await.unwrap();
            backups.get(&backup.id.to_string()).await.unwrap()
        }
    };

    let first = take(vec![BackupContent::Configuration]).await;
    assert_eq!(first.state, BackupState::Completed);
    let failed = take(vec![BackupContent::Certificates]).await;
    assert_eq!(
        failed.state,
        BackupState::Failed,
        "the tests keep no automation.db"
    );
    assert_eq!(failed.failure.as_ref().unwrap().code.as_str(), "NOT_FOUND");
    let refused = backups.archive(&failed.id.to_string()).await.unwrap_err();
    assert_eq!(refused.code.as_str(), "CONFLICT");

    let third = take(vec![BackupContent::Databases]).await;
    assert_eq!(third.state, BackupState::Completed);
    let listed: Vec<Uuid> = backups
        .list()
        .await
        .unwrap()
        .iter()
        .map(|backup| backup.id)
        .collect();
    assert_eq!(
        listed,
        [third.id, failed.id],
        "only the newest two are kept"
    );
    assert!(!database
        .directory()
        .join("backups")
        .join(format!("{}.tar.zst", first.id))
        .exists());

    let pending = backups
        .create(
            &context(),
            BackupRequest {
                contents: vec![BackupContent::Configuration],
                ..BackupRequest::default()
            },
        )
        .await
        .unwrap();
    let busy = backups.delete(&pending.id.to_string()).await.unwrap_err();
    assert_eq!(busy.code.as_str(), "CONFLICT");
    backups.delete(&third.id.to_string()).await.unwrap();
    assert_eq!(
        backups
            .get(&third.id.to_string())
            .await
            .unwrap_err()
            .code
            .as_str(),
        "NOT_FOUND"
    );
}

#[tokio::test]
async fn requests_are_checked_before_anything_is_listed() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    let without_sites = backups(&database, None, 10).await;
    for (request, code) in [
        (BackupRequest::default(), "INVALID_ARGUMENT"),
        (
            BackupRequest {
                contents: vec![BackupContent::Sites],
                ..BackupRequest::default()
            },
            "UNSUPPORTED_CAPABILITY",
        ),
        (
            BackupRequest {
                contents: vec![BackupContent::Certificates],
                site_path: "shop".to_owned(),
                ..BackupRequest::default()
            },
            "INVALID_ARGUMENT",
        ),
        (
            BackupRequest {
                contents: vec![BackupContent::Certificates],
                attachments: BTreeMap::from([("configuration/draft.json".to_owned(), Vec::new())]),
                ..BackupRequest::default()
            },
            "INVALID_ARGUMENT",
        ),
        (
            BackupRequest {
                contents: vec![BackupContent::Configuration],
                attachments: BTreeMap::from([("../escape.json".to_owned(), Vec::new())]),
                ..BackupRequest::default()
            },
            "INVALID_ARGUMENT",
        ),
    ] {
        let refused = without_sites.create(&context(), request).await.unwrap_err();
        assert_eq!(refused.code.as_str(), code, "{refused}");
    }
    assert!(without_sites.list().await.unwrap().is_empty());
    assert_eq!(queued(&database).await, 0);
}

/// What a stand-in target keeps: each archive as it was described, with
/// its bytes.
type Kept = BTreeMap<String, (plugin_contracts::v1::ArchiveInfo, Vec<u8>)>;

/// A plugin's backup target that keeps archives in memory, checking each
/// against the size and digest it was told.
#[derive(Clone, Default)]
struct Target {
    kept: Arc<std::sync::Mutex<Kept>>,
}

type Chunks = std::pin::Pin<
    Box<
        dyn tokio_stream::Stream<
                Item = std::result::Result<
                    plugin_contracts::v1::BackupTargetGetResponse,
                    tonic::Status,
                >,
            > + Send,
    >,
>;

#[tonic::async_trait]
impl plugin_contracts::v1::backup_target_server::BackupTarget for Target {
    async fn put(
        &self,
        request: tonic::Request<tonic::Streaming<plugin_contracts::v1::BackupTargetPutRequest>>,
    ) -> std::result::Result<
        tonic::Response<plugin_contracts::v1::BackupTargetPutResponse>,
        tonic::Status,
    > {
        use plugin_contracts::v1::backup_target_put_request::Part;
        let mut parts = request.into_inner();
        let Some(Part::Archive(info)) = parts.message().await?.and_then(|first| first.part) else {
            return Err(tonic::Status::invalid_argument(
                "describe the archive first",
            ));
        };
        let mut bytes = Vec::new();
        while let Some(part) = parts.message().await? {
            if let Some(Part::Chunk(chunk)) = part.part {
                bytes.extend(chunk);
            }
        }
        if bytes.len() as u64 != info.size || hex::encode(Sha256::digest(&bytes)) != info.sha256 {
            return Err(tonic::Status::data_loss(
                "the archive is not the one described",
            ));
        }
        self.kept
            .lock()
            .unwrap()
            .insert(info.name.clone(), (info, bytes));
        Ok(tonic::Response::new(
            plugin_contracts::v1::BackupTargetPutResponse {},
        ))
    }

    async fn list(
        &self,
        _: tonic::Request<plugin_contracts::v1::BackupTargetListRequest>,
    ) -> std::result::Result<
        tonic::Response<plugin_contracts::v1::BackupTargetListResponse>,
        tonic::Status,
    > {
        let archives = self
            .kept
            .lock()
            .unwrap()
            .values()
            .map(|(info, _)| info.clone())
            .collect();
        Ok(tonic::Response::new(
            plugin_contracts::v1::BackupTargetListResponse { archives },
        ))
    }

    type GetStream = Chunks;

    async fn get(
        &self,
        request: tonic::Request<plugin_contracts::v1::BackupTargetGetRequest>,
    ) -> std::result::Result<tonic::Response<Chunks>, tonic::Status> {
        let name = request.into_inner().name;
        let (_, bytes) = self
            .kept
            .lock()
            .unwrap()
            .get(&name)
            .cloned()
            .ok_or_else(|| tonic::Status::not_found(format!("there is no archive {name}")))?;
        let chunks: Vec<_> = bytes
            .chunks(7)
            .map(|chunk| {
                Ok(plugin_contracts::v1::BackupTargetGetResponse {
                    chunk: chunk.to_vec(),
                })
            })
            .collect();
        Ok(tonic::Response::new(Box::pin(tokio_stream::iter(chunks))))
    }

    async fn delete(
        &self,
        request: tonic::Request<plugin_contracts::v1::BackupTargetDeleteRequest>,
    ) -> std::result::Result<
        tonic::Response<plugin_contracts::v1::BackupTargetDeleteResponse>,
        tonic::Status,
    > {
        self.kept.lock().unwrap().remove(&request.into_inner().name);
        Ok(tonic::Response::new(
            plugin_contracts::v1::BackupTargetDeleteResponse {},
        ))
    }
}

async fn target_channel(target: Target) -> tonic::transport::Channel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(
                plugin_contracts::v1::backup_target_server::BackupTargetServer::new(target),
            )
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_lazy()
}

#[tokio::test]
async fn backups_are_copied_to_targets_and_fetched_back_as_backups() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    configuration(database.directory()).await;
    let sites = sites();
    let target = Target::default();
    let backups = backups(&database, Some(sites.path()), 10)
        .await
        .with_plugins(target_channel(target.clone()).await);
    let requested = backups
        .create(
            &context(),
            BackupRequest {
                contents: vec![BackupContent::Sites],
                site_path: "shop".into(),
                attachments: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    backups.take(requested.id).await.unwrap();
    let taken = backups.get(&requested.id.to_string()).await.unwrap();

    let copy = backups.copy_to(&taken.id.to_string(), "s3").await.unwrap();
    assert_eq!(
        copy.name,
        format!("pingora-panel-backup-{}.tar.zst", taken.id)
    );
    assert_eq!(
        (copy.size_bytes, copy.sha256.as_str()),
        (taken.size_bytes, taken.sha256.as_str())
    );
    let listed = backups.target_archives("s3").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, copy.name);
    assert!(listed[0].created_at.is_some());

    let imported = backups.import(&context(), "s3", &copy.name).await.unwrap();
    assert_ne!(imported.id, taken.id);
    assert_eq!(imported.state, BackupState::Completed);
    assert_eq!(imported.contents, [BackupContent::Sites]);
    assert_eq!(
        (imported.size_bytes, imported.sha256.as_str()),
        (taken.size_bytes, taken.sha256.as_str())
    );
    assert_eq!(imported.requested_by, "alice");
    fs::remove_dir_all(sites.path().join("shop")).unwrap();
    backups
        .restore_sites(&imported.id.to_string(), "shop")
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(sites.path().join("shop/index.html")).unwrap(),
        "<h1>Shop</h1>\n"
    );

    for refused in ["../escape", "", "a/b"] {
        assert_eq!(
            backups
                .import(&context(), "s3", refused)
                .await
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }
    assert_eq!(
        backups
            .import(&context(), "s3", "missing.tar.zst")
            .await
            .unwrap_err()
            .code
            .as_str(),
        panel_errors::ErrorCode::NOT_FOUND
    );
    {
        let mut kept = target.kept.lock().unwrap();
        let (_, bytes) = kept.get_mut(&copy.name).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
    }
    let before = backups.list().await.unwrap().len();
    let damaged = backups
        .import(&context(), "s3", &copy.name)
        .await
        .unwrap_err();
    assert_eq!(
        damaged.code.as_str(),
        panel_errors::ErrorCode::VALIDATION_FAILED
    );
    assert_eq!(backups.list().await.unwrap().len(), before);
    let left: Vec<_> = fs::read_dir(database.directory().join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.ends_with(".tar.zst"))
        .collect();
    assert!(
        left.is_empty(),
        "a damaged archive leaves nothing: {left:?}"
    );

    backups.delete_from("s3", &copy.name).await.unwrap();
    assert!(backups.target_archives("s3").await.unwrap().is_empty());
    let unreachable = backups
        .copy_to(&taken.id.to_string(), "Not A Plugin")
        .await
        .unwrap_err();
    assert_eq!(
        unreachable.code.as_str(),
        panel_errors::ErrorCode::INVALID_ARGUMENT
    );
}
