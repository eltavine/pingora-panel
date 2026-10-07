//! `BackupsPort` over the automation module's `Backups` service.

use crate::AutomationClient;
use async_trait::async_trait;
use futures_util::StreamExt;
use panel_application::{
    Backup, BackupContent, BackupDownload, BackupRequest, BackupState, BackupsPort, CommandContext,
    RequestScope, SitesRestored, TargetArchive,
};
use panel_contracts::automation::v1::{
    self as wire, backups_client::BackupsClient, backups_download_response::Message,
};
use panel_errors::{PanelError, Result};
use panel_service::{
    command_context, propagate_trace, request_context, response_error, status_error,
};
use std::time::SystemTime;
use tonic::transport::Channel;

/// Attachments and members are up to 16 MiB each.
const MOST_MESSAGE_BYTES: usize = 40 * 1024 * 1024;

impl AutomationClient {
    fn backups(&self) -> BackupsClient<Channel> {
        BackupsClient::new(self.channel.clone())
            .max_decoding_message_size(MOST_MESSAGE_BYTES)
            .max_encoding_message_size(MOST_MESSAGE_BYTES)
    }

    /// A request with no deadline, for work the service bounds itself.
    fn lasting<T>(message: T, scope: &RequestScope) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        propagate_trace(request.metadata_mut(), scope.trace_context());
        request
    }
}

fn content_of(value: i32) -> Option<BackupContent> {
    match wire::BackupContent::try_from(value).ok()? {
        wire::BackupContent::Configuration => Some(BackupContent::Configuration),
        wire::BackupContent::Certificates => Some(BackupContent::Certificates),
        wire::BackupContent::Databases => Some(BackupContent::Databases),
        wire::BackupContent::Sites => Some(BackupContent::Sites),
        wire::BackupContent::Unspecified => None,
    }
}

fn wire_content(content: BackupContent) -> i32 {
    (match content {
        BackupContent::Configuration => wire::BackupContent::Configuration,
        BackupContent::Certificates => wire::BackupContent::Certificates,
        BackupContent::Databases => wire::BackupContent::Databases,
        BackupContent::Sites => wire::BackupContent::Sites,
        _ => wire::BackupContent::Unspecified,
    }) as i32
}

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    value.and_then(|value| SystemTime::try_from(value).ok())
}

fn backup_of(backup: wire::Backup) -> Result<Backup> {
    let state = match wire::BackupState::try_from(backup.state) {
        Ok(wire::BackupState::Pending) => BackupState::Pending,
        Ok(wire::BackupState::Running) => BackupState::Running,
        Ok(wire::BackupState::Completed) => BackupState::Completed,
        Ok(wire::BackupState::Failed) => BackupState::Failed,
        _ => {
            return Err(PanelError::unavailable(format!(
                "backup {} is in a state this version does not know",
                backup.id
            )))
        }
    };
    Ok(Backup {
        contents: backup.contents.into_iter().filter_map(content_of).collect(),
        state,
        requested_at: time(backup.requested_at).unwrap_or(SystemTime::UNIX_EPOCH),
        finished_at: time(backup.finished_at),
        failure: backup.failure.map(PanelError::from),
        id: backup.id,
        site_path: backup.site_path,
        requested_by: backup.requested_by,
        size_bytes: backup.size_bytes,
        sha256: backup.sha256,
        files: backup.files,
        product_version: backup.product_version,
    })
}

fn listed(backup: Option<wire::Backup>) -> Result<Backup> {
    backup_of(backup.ok_or_else(|| PanelError::unavailable("the backup was not described"))?)
}

#[async_trait]
impl BackupsPort for AutomationClient {
    async fn list(&self, scope: RequestScope) -> Result<Vec<Backup>> {
        let message = wire::BackupsListRequest {
            context: Some(request_context(&scope)),
        };
        let response = self
            .backups()
            .list(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response.backups.into_iter().map(backup_of).collect()
    }

    async fn get(&self, scope: RequestScope, id: &str) -> Result<Backup> {
        let message = wire::BackupsGetRequest {
            context: Some(request_context(&scope)),
            id: id.to_owned(),
        };
        let response = self
            .backups()
            .get(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        listed(response.backup)
    }

    async fn create(&self, context: CommandContext, request: BackupRequest) -> Result<Backup> {
        let scope = context.scope();
        let message = wire::BackupsCreateRequest {
            context: Some(command_context(&context)),
            contents: request.contents.into_iter().map(wire_content).collect(),
            site_path: request.site_path,
            attachments: request
                .attachments
                .into_iter()
                .map(|(path, content)| wire::BackupAttachment { path, content })
                .collect(),
        };
        let response = self
            .backups()
            .create(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        listed(response.backup)
    }

    async fn delete(&self, context: CommandContext, id: &str) -> Result<()> {
        let scope = context.scope();
        let message = wire::BackupsDeleteRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
        };
        let response = self
            .backups()
            .delete(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)
    }

    async fn download(&self, scope: RequestScope, id: &str) -> Result<BackupDownload> {
        let message = wire::BackupsDownloadRequest {
            context: Some(request_context(&scope)),
            id: id.to_owned(),
        };
        let mut stream = self
            .backups()
            .download(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        let backup = match stream.message().await.map_err(status_error)? {
            Some(wire::BackupsDownloadResponse {
                message: Some(Message::Backup(backup)),
            }) => backup_of(*backup)?,
            Some(wire::BackupsDownloadResponse {
                message: Some(Message::Error(error)),
            }) => return Err(error.into()),
            _ => {
                return Err(PanelError::unavailable(
                    "the download did not begin with the backup",
                ))
            }
        };
        let chunks = stream
            .map(|message| match message.map_err(status_error)?.message {
                Some(Message::Chunk(chunk)) => Ok(chunk),
                Some(Message::Error(error)) => Err(error.into()),
                _ => Err(PanelError::unavailable(
                    "the download sent something other than the archive",
                )),
            })
            .boxed();
        Ok(BackupDownload { backup, chunks })
    }

    async fn member(&self, scope: RequestScope, id: &str, path: &str) -> Result<Vec<u8>> {
        let message = wire::BackupsMemberRequest {
            context: Some(request_context(&scope)),
            id: id.to_owned(),
            path: path.to_owned(),
        };
        let response = self
            .backups()
            .member(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response.content)
    }

    async fn restore_sites(
        &self,
        context: CommandContext,
        id: &str,
        site_path: &str,
    ) -> Result<SitesRestored> {
        let scope = context.scope();
        let message = wire::BackupsRestoreSitesRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            site_path: site_path.to_owned(),
        };
        let response = self
            .backups()
            .restore_sites(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(SitesRestored {
            files: response.files,
            bytes: response.bytes,
        })
    }

    async fn copy_to_target(
        &self,
        context: CommandContext,
        id: &str,
        target: &str,
    ) -> Result<TargetArchive> {
        let scope = context.scope();
        let message = wire::BackupsCopyToTargetRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            target: target.to_owned(),
        };
        let response = self
            .backups()
            .copy_to_target(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response
            .archive
            .map(target_archive)
            .ok_or_else(|| PanelError::internal("the copy was not described"))
    }

    async fn target_archives(
        &self,
        scope: RequestScope,
        target: &str,
    ) -> Result<Vec<TargetArchive>> {
        let message = wire::BackupsListTargetRequest {
            context: Some(request_context(&scope)),
            target: target.to_owned(),
        };
        let response = self
            .backups()
            .list_target(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response.archives.into_iter().map(target_archive).collect())
    }

    async fn import_from_target(
        &self,
        context: CommandContext,
        target: &str,
        name: &str,
    ) -> Result<Backup> {
        let scope = context.scope();
        let message = wire::BackupsImportFromTargetRequest {
            context: Some(command_context(&context)),
            target: target.to_owned(),
            name: name.to_owned(),
        };
        let response = self
            .backups()
            .import_from_target(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        backup_of(
            response
                .backup
                .ok_or_else(|| PanelError::internal("the imported backup was not described"))?,
        )
    }

    async fn delete_from_target(
        &self,
        context: CommandContext,
        target: &str,
        name: &str,
    ) -> Result<()> {
        let scope = context.scope();
        let message = wire::BackupsDeleteFromTargetRequest {
            context: Some(command_context(&context)),
            target: target.to_owned(),
            name: name.to_owned(),
        };
        let response = self
            .backups()
            .delete_from_target(Self::lasting(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)
    }
}

fn target_archive(archive: wire::TargetArchive) -> TargetArchive {
    TargetArchive {
        name: archive.name,
        size_bytes: archive.size_bytes,
        sha256: archive.sha256,
        created_at: time(archive.created_at),
    }
}
