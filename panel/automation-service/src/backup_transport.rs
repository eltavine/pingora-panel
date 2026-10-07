//! The backups over gRPC: requests decode into the backups' terms and what
//! they answer encodes back. Application failures travel in each response's
//! error, transport failures as status codes.

use crate::backups::{Backup, BackupContent, BackupRequest, BackupState, Backups, TargetArchive};
use futures_util::{stream::BoxStream, StreamExt};
use panel_contracts::automation::v1::{self as wire, backups_server};
use panel_errors::{PanelError, Result};
use panel_service::{decode_command, decode_scope, trace_context};
use std::time::SystemTime;
use tokio_util::io::ReaderStream;
use tonic::{Request, Response, Status};

const CHUNK_BYTES: usize = 64 * 1024;

/// Serves [`Backups`] over gRPC.
pub struct BackupsTransport {
    backups: Backups,
}

impl BackupsTransport {
    pub fn new(backups: Backups) -> Self {
        Self { backups }
    }
}

fn content(value: i32) -> Result<BackupContent> {
    match wire::BackupContent::try_from(value) {
        Ok(wire::BackupContent::Configuration) => Ok(BackupContent::Configuration),
        Ok(wire::BackupContent::Certificates) => Ok(BackupContent::Certificates),
        Ok(wire::BackupContent::Databases) => Ok(BackupContent::Databases),
        Ok(wire::BackupContent::Sites) => Ok(BackupContent::Sites),
        _ => Err(PanelError::invalid_argument(format!(
            "backup content {value} is not known here"
        ))),
    }
}

fn encoded_content(content: BackupContent) -> wire::BackupContent {
    match content {
        BackupContent::Configuration => wire::BackupContent::Configuration,
        BackupContent::Certificates => wire::BackupContent::Certificates,
        BackupContent::Databases => wire::BackupContent::Databases,
        BackupContent::Sites => wire::BackupContent::Sites,
    }
}

fn encoded_archive(archive: TargetArchive) -> wire::TargetArchive {
    wire::TargetArchive {
        name: archive.name,
        size_bytes: archive.size_bytes,
        sha256: archive.sha256,
        created_at: archive.created_at.map(|at| SystemTime::from(at).into()),
    }
}

fn encoded(backup: &Backup) -> wire::Backup {
    wire::Backup {
        id: backup.id.to_string(),
        contents: backup
            .contents
            .iter()
            .map(|&content| encoded_content(content) as i32)
            .collect(),
        site_path: backup.site_path.clone(),
        state: match backup.state {
            BackupState::Pending => wire::BackupState::Pending,
            BackupState::Running => wire::BackupState::Running,
            BackupState::Completed => wire::BackupState::Completed,
            BackupState::Failed => wire::BackupState::Failed,
        } as i32,
        requested_by: backup.requested_by.clone(),
        requested_at: Some(SystemTime::from(backup.requested_at).into()),
        finished_at: backup
            .finished_at
            .map(|finished| SystemTime::from(finished).into()),
        size_bytes: backup.size_bytes,
        sha256: backup.sha256.clone(),
        files: backup.files,
        failure: backup.failure.as_ref().map(Into::into),
        product_version: backup.product_version.clone(),
    }
}

#[tonic::async_trait]
impl backups_server::Backups for BackupsTransport {
    async fn list(
        &self,
        request: Request<wire::BackupsListRequest>,
    ) -> std::result::Result<Response<wire::BackupsListResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let listed = async {
            decode_scope(request.context, trace)?;
            self.backups.list().await
        }
        .await;
        Ok(Response::new(match listed {
            Ok(backups) => wire::BackupsListResponse {
                backups: backups.iter().map(encoded).collect(),
                error: None,
            },
            Err(error) => wire::BackupsListResponse {
                backups: Vec::new(),
                error: Some((&error).into()),
            },
        }))
    }

    async fn get(
        &self,
        request: Request<wire::BackupsGetRequest>,
    ) -> std::result::Result<Response<wire::BackupsGetResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let found = async {
            decode_scope(request.context, trace)?;
            self.backups.get(&request.id).await
        }
        .await;
        Ok(Response::new(match found {
            Ok(backup) => wire::BackupsGetResponse {
                backup: Some(encoded(&backup)),
                error: None,
            },
            Err(error) => wire::BackupsGetResponse {
                backup: None,
                error: Some((&error).into()),
            },
        }))
    }

    async fn create(
        &self,
        request: Request<wire::BackupsCreateRequest>,
    ) -> std::result::Result<Response<wire::BackupsCreateResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let created = async {
            let context = decode_command(request.context, trace)?;
            let wanted = BackupRequest {
                contents: request
                    .contents
                    .into_iter()
                    .map(content)
                    .collect::<Result<_>>()?,
                site_path: request.site_path,
                attachments: request
                    .attachments
                    .into_iter()
                    .map(|attachment| (attachment.path, attachment.content))
                    .collect(),
            };
            self.backups.create(&context, wanted).await
        }
        .await;
        Ok(Response::new(match created {
            Ok(backup) => wire::BackupsCreateResponse {
                backup: Some(encoded(&backup)),
                error: None,
            },
            Err(error) => wire::BackupsCreateResponse {
                backup: None,
                error: Some((&error).into()),
            },
        }))
    }

    async fn delete(
        &self,
        request: Request<wire::BackupsDeleteRequest>,
    ) -> std::result::Result<Response<wire::BackupsDeleteResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let deleted = async {
            decode_command(request.context, trace)?;
            self.backups.delete(&request.id).await
        }
        .await;
        Ok(Response::new(wire::BackupsDeleteResponse {
            error: deleted.err().map(|error| (&error).into()),
        }))
    }

    type DownloadStream =
        BoxStream<'static, std::result::Result<wire::BackupsDownloadResponse, Status>>;

    async fn download(
        &self,
        request: Request<wire::BackupsDownloadRequest>,
    ) -> std::result::Result<Response<Self::DownloadStream>, Status> {
        use wire::backups_download_response::Message;
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let opened = async {
            decode_scope(request.context, trace)?;
            let (backup, archive) = self.backups.archive(&request.id).await?;
            let file = tokio::fs::File::open(&archive).await.map_err(|error| {
                PanelError::storage_unavailable(format!("cannot read the archive: {error}"))
            })?;
            Ok::<_, PanelError>((backup, file))
        }
        .await;
        let message = |message| {
            Ok(wire::BackupsDownloadResponse {
                message: Some(message),
            })
        };
        let stream = match opened {
            Ok((backup, file)) => futures_util::stream::once(async move {
                message(Message::Backup(Box::new(encoded(&backup))))
            })
            .chain(
                ReaderStream::with_capacity(file, CHUNK_BYTES).map(move |chunk| match chunk {
                    Ok(chunk) => message(Message::Chunk(chunk.to_vec())),
                    Err(error) => message(Message::Error(
                        (&PanelError::storage_unavailable(format!(
                            "cannot read the archive: {error}"
                        )))
                            .into(),
                    )),
                }),
            )
            .boxed(),
            Err(error) => {
                futures_util::stream::once(async move { message(Message::Error((&error).into())) })
                    .boxed()
            }
        };
        Ok(Response::new(stream))
    }

    async fn member(
        &self,
        request: Request<wire::BackupsMemberRequest>,
    ) -> std::result::Result<Response<wire::BackupsMemberResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let read = async {
            decode_scope(request.context, trace)?;
            self.backups.member(&request.id, &request.path).await
        }
        .await;
        Ok(Response::new(match read {
            Ok(content) => wire::BackupsMemberResponse {
                content,
                error: None,
            },
            Err(error) => wire::BackupsMemberResponse {
                content: Vec::new(),
                error: Some((&error).into()),
            },
        }))
    }

    async fn restore_sites(
        &self,
        request: Request<wire::BackupsRestoreSitesRequest>,
    ) -> std::result::Result<Response<wire::BackupsRestoreSitesResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let restored = async {
            decode_command(request.context, trace)?;
            self.backups
                .restore_sites(&request.id, &request.site_path)
                .await
        }
        .await;
        Ok(Response::new(match restored {
            Ok(extraction) => wire::BackupsRestoreSitesResponse {
                files: extraction.files,
                bytes: extraction.bytes,
                error: None,
            },
            Err(error) => wire::BackupsRestoreSitesResponse {
                error: Some((&error).into()),
                ..wire::BackupsRestoreSitesResponse::default()
            },
        }))
    }

    async fn copy_to_target(
        &self,
        request: Request<wire::BackupsCopyToTargetRequest>,
    ) -> std::result::Result<Response<wire::BackupsCopyToTargetResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let copied = async {
            decode_command(request.context, trace)?;
            self.backups.copy_to(&request.id, &request.target).await
        }
        .await;
        Ok(Response::new(match copied {
            Ok(archive) => wire::BackupsCopyToTargetResponse {
                archive: Some(encoded_archive(archive)),
                error: None,
            },
            Err(error) => wire::BackupsCopyToTargetResponse {
                archive: None,
                error: Some((&error).into()),
            },
        }))
    }

    async fn list_target(
        &self,
        request: Request<wire::BackupsListTargetRequest>,
    ) -> std::result::Result<Response<wire::BackupsListTargetResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let listed = async {
            decode_scope(request.context, trace)?;
            self.backups.target_archives(&request.target).await
        }
        .await;
        Ok(Response::new(match listed {
            Ok(archives) => wire::BackupsListTargetResponse {
                archives: archives.into_iter().map(encoded_archive).collect(),
                error: None,
            },
            Err(error) => wire::BackupsListTargetResponse {
                archives: Vec::new(),
                error: Some((&error).into()),
            },
        }))
    }

    async fn import_from_target(
        &self,
        request: Request<wire::BackupsImportFromTargetRequest>,
    ) -> std::result::Result<Response<wire::BackupsImportFromTargetResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let imported = async {
            let context = decode_command(request.context, trace)?;
            self.backups
                .import(&context, &request.target, &request.name)
                .await
        }
        .await;
        Ok(Response::new(match imported {
            Ok(backup) => wire::BackupsImportFromTargetResponse {
                backup: Some(encoded(&backup)),
                error: None,
            },
            Err(error) => wire::BackupsImportFromTargetResponse {
                backup: None,
                error: Some((&error).into()),
            },
        }))
    }

    async fn delete_from_target(
        &self,
        request: Request<wire::BackupsDeleteFromTargetRequest>,
    ) -> std::result::Result<Response<wire::BackupsDeleteFromTargetResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let deleted = async {
            decode_command(request.context, trace)?;
            self.backups
                .delete_from(&request.target, &request.name)
                .await
        }
        .await;
        Ok(Response::new(wire::BackupsDeleteFromTargetResponse {
            error: deleted.err().map(|error| (&error).into()),
        }))
    }
}
