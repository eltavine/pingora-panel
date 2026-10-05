#![forbid(unsafe_code)]

use automation_grpc_client::AutomationClient;
use futures_util::{stream::BoxStream, StreamExt};
use panel_application::{
    BackupContent, BackupRequest, BackupState, BackupsPort, CommandContext, RequestScope,
};
use panel_context::{IdempotencyKey, RequestDeadline, RequestId};
use panel_contracts::{
    automation::v1::{
        self as wire,
        backups_download_response::Message,
        backups_server::{Backups, BackupsServer},
    },
    common::v1 as common,
};
use std::time::{Duration, SystemTime};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};

fn taken() -> wire::Backup {
    wire::Backup {
        id: "6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4".into(),
        contents: vec![
            wire::BackupContent::Configuration as i32,
            wire::BackupContent::Sites as i32,
            99,
        ],
        site_path: "shop".into(),
        state: wire::BackupState::Completed as i32,
        requested_by: "alice".into(),
        requested_at: Some((SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)).into()),
        finished_at: Some((SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060)).into()),
        size_bytes: 6,
        sha256: "ab".repeat(32),
        files: 3,
        failure: None,
        product_version: "1.2.3".into(),
    }
}

fn missing() -> common::Error {
    common::Error {
        code: "NOT_FOUND".into(),
        message: "backup b-2 does not exist".into(),
        ..common::Error::default()
    }
}

struct FakeBackups;

#[tonic::async_trait]
impl Backups for FakeBackups {
    async fn list(
        &self,
        _: Request<wire::BackupsListRequest>,
    ) -> Result<Response<wire::BackupsListResponse>, Status> {
        Ok(Response::new(wire::BackupsListResponse {
            backups: vec![taken()],
            error: None,
        }))
    }

    async fn get(
        &self,
        request: Request<wire::BackupsGetRequest>,
    ) -> Result<Response<wire::BackupsGetResponse>, Status> {
        let found = request.into_inner().id == taken().id;
        Ok(Response::new(wire::BackupsGetResponse {
            backup: found.then(taken),
            error: (!found).then(missing),
        }))
    }

    async fn create(
        &self,
        request: Request<wire::BackupsCreateRequest>,
    ) -> Result<Response<wire::BackupsCreateResponse>, Status> {
        let request = request.into_inner();
        assert_eq!(request.context.unwrap().actor, "alice");
        assert_eq!(
            request.contents,
            [wire::BackupContent::Configuration as i32]
        );
        assert_eq!(request.attachments[0].path, "configuration/draft.json");
        Ok(Response::new(wire::BackupsCreateResponse {
            backup: Some(wire::Backup {
                state: wire::BackupState::Pending as i32,
                ..taken()
            }),
            error: None,
        }))
    }

    async fn delete(
        &self,
        _: Request<wire::BackupsDeleteRequest>,
    ) -> Result<Response<wire::BackupsDeleteResponse>, Status> {
        Ok(Response::new(wire::BackupsDeleteResponse {
            error: Some(missing()),
        }))
    }

    type DownloadStream = BoxStream<'static, Result<wire::BackupsDownloadResponse, Status>>;

    async fn download(
        &self,
        _: Request<wire::BackupsDownloadRequest>,
    ) -> Result<Response<Self::DownloadStream>, Status> {
        let messages = [
            Message::Backup(Box::new(taken())),
            Message::Chunk(b"tar".to_vec()),
            Message::Chunk(b".zst".to_vec()),
        ];
        Ok(Response::new(
            futures_util::stream::iter(messages.map(|message| {
                Ok(wire::BackupsDownloadResponse {
                    message: Some(message),
                })
            }))
            .boxed(),
        ))
    }

    async fn member(
        &self,
        request: Request<wire::BackupsMemberRequest>,
    ) -> Result<Response<wire::BackupsMemberResponse>, Status> {
        Ok(Response::new(wire::BackupsMemberResponse {
            content: request.into_inner().path.into_bytes(),
            error: None,
        }))
    }

    async fn restore_sites(
        &self,
        request: Request<wire::BackupsRestoreSitesRequest>,
    ) -> Result<Response<wire::BackupsRestoreSitesResponse>, Status> {
        assert_eq!(request.into_inner().site_path, "shop");
        Ok(Response::new(wire::BackupsRestoreSitesResponse {
            files: 2,
            bytes: 30,
            error: None,
        }))
    }
}

async fn client() -> AutomationClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(
        Server::builder()
            .add_service(BackupsServer::new(FakeBackups))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    AutomationClient::from_channel(
        Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect_lazy(),
    )
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("request-1").unwrap())
}

fn context() -> CommandContext {
    CommandContext::new(
        RequestId::new("request-1").unwrap(),
        RequestId::new("request-1").unwrap(),
        "alice",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("key-1").unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn backups_reach_the_application_in_its_terms() {
    let client = client().await;
    let listed = client.list(scope()).await.unwrap();
    let backup = &listed[0];
    assert_eq!(
        backup.contents,
        [BackupContent::Configuration, BackupContent::Sites],
        "contents this version does not know are left out"
    );
    assert_eq!(backup.state, BackupState::Completed);
    assert_eq!(
        backup.finished_at.unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060)
    );

    let refused = client.get(scope(), "b-2").await.unwrap_err();
    assert_eq!(refused.code.as_str(), "NOT_FOUND");
    let refused = client.delete(context(), "b-2").await.unwrap_err();
    assert_eq!(refused.code.as_str(), "NOT_FOUND");

    let created = client
        .create(
            context(),
            BackupRequest {
                contents: vec![BackupContent::Configuration],
                attachments: [("configuration/draft.json".to_owned(), b"{}".to_vec())].into(),
                ..BackupRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(created.state, BackupState::Pending);

    let download = client.download(scope(), &backup.id).await.unwrap();
    assert_eq!(download.backup.id, backup.id);
    let archive: Vec<u8> = download.chunks.map(|chunk| chunk.unwrap()).concat().await;
    assert_eq!(archive, b"tar.zst");

    assert_eq!(
        client
            .member(scope(), &backup.id, "configuration/active.json")
            .await
            .unwrap(),
        b"configuration/active.json"
    );
    let restored = client
        .restore_sites(context(), &backup.id, "shop")
        .await
        .unwrap();
    assert_eq!((restored.files, restored.bytes), (2, 30));
}
