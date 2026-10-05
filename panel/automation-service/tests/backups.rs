#![forbid(unsafe_code)]

use automation_service::{Backups, BackupsTransport, MIGRATIONS};
use futures_util::StreamExt;
use panel_contracts::{
    automation::v1::{
        self as wire, backups_download_response::Message, backups_server::Backups as _,
    },
    common::v1 as common,
};
use panel_jobs::MemoryJobStore;
use panel_sqlite::testing::TestDatabase;
use sha2::{Digest, Sha256};
use std::{fs, sync::Arc};
use tonic::Request;

fn context() -> Option<common::RequestContext> {
    Some(common::RequestContext {
        request_id: "request-1".into(),
        actor: "alice".into(),
        deadline: "2099-01-01T00:00:00Z".into(),
        idempotency_key: "key-1".into(),
        ..common::RequestContext::default()
    })
}

#[tokio::test]
async fn backups_are_taken_downloaded_restored_and_removed_over_grpc() {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    let sites = tempfile::tempdir().unwrap();
    fs::create_dir_all(sites.path().join("shop")).unwrap();
    fs::write(sites.path().join("shop/index.html"), "<h1>Shop</h1>\n").unwrap();
    let backups = Backups::new(
        database.database().clone(),
        Arc::new(MemoryJobStore::default()),
        database.directory(),
        Some(sites.path().to_owned()),
        10,
        "1.2.3",
    );
    let transport = BackupsTransport::new(backups.clone());

    let created = transport
        .create(Request::new(wire::BackupsCreateRequest {
            context: context(),
            contents: vec![wire::BackupContent::Sites as i32],
            site_path: "shop".into(),
            attachments: Vec::new(),
        }))
        .await
        .unwrap()
        .into_inner();
    let backup = created.backup.expect("the backup is listed");
    assert_eq!(backup.state, wire::BackupState::Pending as i32);
    assert_eq!(backup.requested_by, "alice");
    backups.take(backup.id.parse().unwrap()).await.unwrap();

    let listed = transport
        .list(Request::new(wire::BackupsListRequest {
            context: context(),
        }))
        .await
        .unwrap()
        .into_inner();
    let taken = &listed.backups[0];
    assert_eq!(taken.state, wire::BackupState::Completed as i32);
    assert_eq!(taken.contents, [wire::BackupContent::Sites as i32]);
    assert!(taken.finished_at.is_some());

    let mut stream = transport
        .download(Request::new(wire::BackupsDownloadRequest {
            context: context(),
            id: taken.id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(Ok(wire::BackupsDownloadResponse {
        message: Some(Message::Backup(described)),
    })) = stream.next().await
    else {
        panic!("the download begins with the backup");
    };
    assert_eq!(described.id, taken.id);
    let mut archive = Vec::new();
    while let Some(message) = stream.next().await {
        match message.unwrap().message {
            Some(Message::Chunk(chunk)) => archive.extend(chunk),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(archive.len() as u64, taken.size_bytes);
    assert_eq!(hex::encode(Sha256::digest(&archive)), taken.sha256);

    let missing = transport
        .member(Request::new(wire::BackupsMemberRequest {
            context: context(),
            id: taken.id.clone(),
            path: "configuration/draft.json".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(missing.error.unwrap().code, "NOT_FOUND");

    fs::write(sites.path().join("shop/index.html"), "changed").unwrap();
    let restored = transport
        .restore_sites(Request::new(wire::BackupsRestoreSitesRequest {
            context: context(),
            id: taken.id.clone(),
            site_path: "shop".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!((restored.files, restored.bytes), (1, 14));
    assert_eq!(
        fs::read_to_string(sites.path().join("shop/index.html")).unwrap(),
        "<h1>Shop</h1>\n"
    );

    let unknown = transport
        .create(Request::new(wire::BackupsCreateRequest {
            context: context(),
            contents: vec![99],
            ..wire::BackupsCreateRequest::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unknown.error.unwrap().code, "INVALID_ARGUMENT");

    let deleted = transport
        .delete(Request::new(wire::BackupsDeleteRequest {
            context: context(),
            id: taken.id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(deleted.error.is_none());
    let gone = transport
        .get(Request::new(wire::BackupsGetRequest {
            context: context(),
            id: taken.id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(gone.error.unwrap().code, "NOT_FOUND");
}
