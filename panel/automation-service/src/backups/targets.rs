//! Copies of backups that plugins keep through their backup target ports
//! (ADR 0044). An archive goes out as it was taken, and one fetched back
//! becomes a backup of its own once every member checks against its
//! manifest; an archive that does not leaves nothing behind.

use super::{blocking, failure, Backup, BackupContent, Backups, WORKING_PREFIX};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::CommandContext;
use panel_errors::{PanelError, Result};
use panel_sqlite::storage_error;
use plugin_contracts::{
    v1::{
        backup_target_client::BackupTargetClient, backup_target_put_request::Part, ArchiveInfo,
        BackupTargetDeleteRequest, BackupTargetGetRequest, BackupTargetListRequest,
        BackupTargetPutRequest,
    },
    Plugin,
};
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use std::{fs, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use tonic::{codegen::InterceptedService, transport::Channel, Request};
use uuid::Uuid;

/// How much of an archive each message carries.
const CHUNK: usize = 256 * 1024;
/// How long copying or fetching an archive may take.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// An archive a backup target keeps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetArchive {
    pub name: String,
    pub size_bytes: u64,
    /// In lowercase hexadecimal.
    pub sha256: String,
    pub created_at: Option<DateTime<Utc>>,
}

type Target = BackupTargetClient<InterceptedService<Channel, Plugin>>;

fn refused(plugin: &str, what: &str, status: &tonic::Status) -> PanelError {
    let message = format!("plugin {plugin} did not {what}: {}", status.message());
    match status.code() {
        tonic::Code::NotFound => PanelError::not_found(message),
        tonic::Code::InvalidArgument => PanelError::invalid_argument(message),
        tonic::Code::DeadlineExceeded => PanelError::deadline_exceeded(message),
        _ => PanelError::unavailable(message),
    }
}

/// An archive's name as a target keeps it: a plain file name.
fn checked_name(name: &str) -> Result<&str> {
    let plain = !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control);
    if plain {
        Ok(name)
    } else {
        Err(PanelError::invalid_argument(format!(
            "{name:?} is not the name of an archive"
        )))
    }
}

fn time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.to_utc())
}

impl Backups {
    /// Keeps copies with the backup targets of plugins the plugins module
    /// at `plugins` runs.
    pub fn with_plugins(mut self, plugins: Channel) -> Self {
        self.plugins = Some(plugins);
        self
    }

    fn target(&self, plugin: &str) -> Result<Target> {
        if !crate::dns::is_plugin_name(plugin) {
            return Err(PanelError::invalid_argument(format!(
                "{plugin:?} is not a plugin name: lowercase letters, digits and hyphens"
            )));
        }
        let plugins = self.plugins.clone().ok_or_else(|| {
            PanelError::unavailable("plugins cannot be reached from the automation module")
        })?;
        let named = Plugin::named(plugin)
            .map_err(|status| PanelError::invalid_argument(status.message().to_owned()))?;
        Ok(BackupTargetClient::with_interceptor(plugins, named))
    }

    /// Copies a taken backup's archive to the target `plugin` provides, as
    /// `pingora-panel-backup-<id>.tar.zst`, replacing a copy of that name.
    pub async fn copy_to(&self, id: &str, plugin: &str) -> Result<TargetArchive> {
        let mut target = self.target(plugin)?;
        let (backup, path) = self.archive(id).await?;
        let copy = TargetArchive {
            name: format!("pingora-panel-backup-{}.tar.zst", backup.id),
            size_bytes: backup.size_bytes,
            sha256: backup.sha256.clone(),
            created_at: backup.finished_at,
        };
        let file = tokio::fs::File::open(&path).await.map_err(failure)?;
        let info = BackupTargetPutRequest {
            part: Some(Part::Archive(ArchiveInfo {
                name: copy.name.clone(),
                size: copy.size_bytes,
                sha256: copy.sha256.clone(),
                created_at: copy
                    .created_at
                    .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
                    .unwrap_or_default(),
            })),
        };
        // A read that fails ends the stream short, so the target, which
        // checks the size and digest it was told, refuses the copy.
        let chunks = tokio_util::io::ReaderStream::with_capacity(file, CHUNK)
            .map_while(|chunk| chunk.ok())
            .map(|chunk| BackupTargetPutRequest {
                part: Some(Part::Chunk(chunk.to_vec())),
            });
        let mut request = Request::new(tokio_stream::once(info).chain(chunks));
        request.set_timeout(TRANSFER_TIMEOUT);
        target
            .put(request)
            .await
            .map_err(|status| refused(plugin, "keep the copy", &status))?;
        Ok(copy)
    }

    /// The archives the target `plugin` provides keeps.
    pub async fn target_archives(&self, plugin: &str) -> Result<Vec<TargetArchive>> {
        let mut request = Request::new(BackupTargetListRequest {});
        request.set_timeout(CALL_TIMEOUT);
        let listed = self
            .target(plugin)?
            .list(request)
            .await
            .map_err(|status| refused(plugin, "list its archives", &status))?
            .into_inner();
        Ok(listed
            .archives
            .into_iter()
            .map(|archive| TargetArchive {
                name: archive.name,
                size_bytes: archive.size,
                sha256: archive.sha256,
                created_at: time(&archive.created_at),
            })
            .collect())
    }

    /// Removes an archive the target `plugin` provides keeps.
    pub async fn delete_from(&self, plugin: &str, name: &str) -> Result<()> {
        let mut request = Request::new(BackupTargetDeleteRequest {
            name: checked_name(name)?.to_owned(),
        });
        request.set_timeout(CALL_TIMEOUT);
        self.target(plugin)?
            .delete(request)
            .await
            .map_err(|status| refused(plugin, "delete the archive", &status))?;
        Ok(())
    }

    /// Fetches archive `name` from the target `plugin` provides and keeps it
    /// as a backup once every member checks against its manifest.
    pub async fn import(
        &self,
        context: &CommandContext,
        plugin: &str,
        name: &str,
    ) -> Result<Backup> {
        let mut target = self.target(plugin)?;
        let name = checked_name(name)?;
        let id = Uuid::now_v7();
        let directory = self.inner.directory.clone();
        let partial = directory.join(format!("{WORKING_PREFIX}{id}.importing"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(failure)?;
        let mut request = Request::new(BackupTargetGetRequest {
            name: name.to_owned(),
        });
        request.set_timeout(TRANSFER_TIMEOUT);
        let fetched = async {
            let mut chunks = target
                .get(request)
                .await
                .map_err(|status| refused(plugin, "return the archive", &status))?
                .into_inner();
            let mut file = tokio::fs::File::create(&partial).await.map_err(failure)?;
            let (mut size, mut digest) = (0_u64, Sha256::new());
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk
                    .map_err(|status| refused(plugin, "return the whole archive", &status))?
                    .chunk;
                size += chunk.len() as u64;
                digest.update(&chunk);
                file.write_all(&chunk).await.map_err(failure)?;
            }
            file.sync_all().await.map_err(failure)?;
            let checked = partial.clone();
            let manifest = blocking(move || panel_backup::verify(&checked))
                .await
                .map_err(|error| {
                    PanelError::validation_failed(format!(
                        "archive {name} does not check against its manifest: {}",
                        error.message
                    ))
                })?;
            Ok::<_, PanelError>((manifest, size, hex::encode(digest.finalize())))
        }
        .await;
        let (manifest, size, sha256) = match fetched {
            Ok(fetched) => fetched,
            Err(error) => {
                let _ = fs::remove_file(&partial);
                return Err(error);
            }
        };
        let mut contents: Vec<BackupContent> = manifest
            .contents
            .iter()
            .filter_map(|content| BackupContent::parse(content))
            .collect();
        contents.sort();
        contents.dedup();
        if contents.is_empty() {
            let _ = fs::remove_file(&partial);
            return Err(PanelError::validation_failed(format!(
                "archive {name} holds nothing a backup holds"
            )));
        }
        let archive = self.archive_path(id);
        let renamed = partial.clone();
        blocking(move || fs::rename(&renamed, &archive).map_err(failure)).await?;
        let names: Vec<&str> = contents.iter().map(|content| content.as_str()).collect();
        let now = Utc::now();
        let inserted = sqlx::query(
            "INSERT INTO backups (backup_id, contents, site_path, state, requested_by, \
             requested_at, finished_at, size_bytes, sha256, files, product_version) \
             VALUES (?1, ?2, '', 'completed', ?3, ?4, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(id.to_string())
        .bind(Json(&names))
        .bind(context.actor())
        .bind(now)
        .bind(i64::try_from(size).unwrap_or(i64::MAX))
        .bind(&sha256)
        .bind(i64::try_from(manifest.members.len()).unwrap_or(i64::MAX))
        .bind(&manifest.product_version)
        .execute(self.inner.database.pool())
        .await
        .map_err(storage_error);
        if let Err(error) = inserted {
            let _ = fs::remove_file(self.archive_path(id));
            return Err(error);
        }
        self.prune().await?;
        self.get(&id.to_string()).await
    }
}
