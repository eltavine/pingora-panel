//! Backups (ADR 0035): archives of the control plane's databases and the
//! sites' directory, taken by a durable job into the data directory's
//! `backups` directory. Each database is copied with `VACUUM INTO`, a
//! consistent snapshot taken while its module goes on writing; nothing reads
//! another module's tables.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_application::CommandContext;
use panel_backup::Extraction;
use panel_errors::{ErrorCode, PanelError, Result};
use panel_events::IdempotencyKey;
use panel_jobs::{JobContext, JobHandler, JobKind, JobOrigin, JobSpec, JobStore};
use panel_sqlite::{storage_error, ServiceDatabase};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteRow},
    types::Json,
    Connection, Row, SqliteConnection,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// The job that takes a backup.
pub const TAKE_JOB: &str = "backup.take";
/// Where the static sites are, to back them up and restore them.
pub const SITES_ROOT_ENV: &str = "PINGORA_PANEL_SITES_ROOT";
/// How many finished backups are kept; older ones are removed.
pub const KEPT_ENV: &str = "PINGORA_PANEL_BACKUPS_KEPT";
pub const DEFAULT_KEPT: usize = 10;
/// Members up to this size are read whole.
pub const MOST_MEMBER_BYTES: u64 = 16 * 1024 * 1024;
/// Attached members are kept below this directory of an archive.
const ATTACHMENTS: &str = "configuration";
const CONFIGURATION_DATABASE: &str = "config";
/// What the panel names its own entries while it takes or restores backups.
const WORKING_PREFIX: &str = ".pingora-panel-";
const BUSY: Duration = Duration::from_secs(30);
/// The columns [`backup`] reads.
macro_rules! columns {
    () => {
        "backup_id, contents, site_path, state, requested_by, requested_at, finished_at, \
         size_bytes, sha256, files, failure_code, failure_message, product_version"
    };
}

/// What a backup holds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum BackupContent {
    /// The configuration's database, with what the caller attaches.
    Configuration,
    /// The automation module's database: certificates with their keys
    /// sealed, ACME accounts and DNS providers.
    Certificates,
    /// Every module's database.
    Databases,
    /// The sites' directory, or one directory below it.
    Sites,
}

impl BackupContent {
    pub const ALL: [Self; 4] = [
        Self::Configuration,
        Self::Certificates,
        Self::Databases,
        Self::Sites,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Certificates => "certificates",
            Self::Databases => "databases",
            Self::Sites => "sites",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|content| content.as_str() == value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackupState {
    Pending,
    Running,
    Completed,
    Failed,
}

impl BackupState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::Pending, Self::Running, Self::Completed, Self::Failed]
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

/// A backup and how far taking it got.
#[derive(Clone, Debug)]
pub struct Backup {
    pub id: Uuid,
    pub contents: Vec<BackupContent>,
    /// The directory below the sites' directory it holds; empty for all.
    pub site_path: String,
    pub state: BackupState,
    pub requested_by: String,
    pub requested_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The archive's size and SHA-256, once taken.
    pub size_bytes: u64,
    pub sha256: String,
    pub files: u64,
    pub failure: Option<PanelError>,
    pub product_version: String,
}

/// What to back up.
#[derive(Clone, Debug, Default)]
pub struct BackupRequest {
    pub contents: Vec<BackupContent>,
    pub site_path: String,
    /// Members written into the archive as given, below `configuration/`.
    pub attachments: BTreeMap<String, Vec<u8>>,
}

/// The backups of an installation and the archives they left.
#[derive(Clone)]
pub struct Backups {
    inner: Arc<Inner>,
}

struct Inner {
    database: ServiceDatabase,
    jobs: Arc<dyn JobStore>,
    data_directory: PathBuf,
    directory: PathBuf,
    sites: Option<PathBuf>,
    kept: usize,
    release: String,
    restoring: Mutex<()>,
}

impl Backups {
    /// Backups of the databases in `data_directory`, kept in its `backups`
    /// directory, and of the sites in `sites` when given.
    pub fn new(
        database: ServiceDatabase,
        jobs: Arc<dyn JobStore>,
        data_directory: &Path,
        sites: Option<PathBuf>,
        kept: usize,
        release: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                database,
                jobs,
                data_directory: data_directory.to_owned(),
                directory: data_directory.join("backups"),
                sites,
                kept: kept.max(1),
                release: release.into(),
                restoring: Mutex::new(()),
            }),
        }
    }

    /// Every backup, newest first.
    pub async fn list(&self) -> Result<Vec<Backup>> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM backups ORDER BY requested_at DESC, backup_id"
        ))
        .fetch_all(self.inner.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(backup)
        .collect()
    }

    pub async fn get(&self, id: &str) -> Result<Backup> {
        let id = parse_id(id)?;
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM backups WHERE backup_id = ?1"
        ))
        .bind(id.to_string())
        .fetch_optional(self.inner.database.pool())
        .await
        .map_err(storage_error)?
        .as_ref()
        .map(backup)
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("backup {id} does not exist")))
    }

    /// Lists a backup as pending and takes it in the background.
    pub async fn create(&self, context: &CommandContext, request: BackupRequest) -> Result<Backup> {
        let mut contents = request.contents;
        contents.sort();
        contents.dedup();
        if contents.is_empty() {
            return Err(PanelError::invalid_argument("say what the backup holds"));
        }
        let sites = contents.contains(&BackupContent::Sites);
        if sites && self.inner.sites.is_none() {
            return Err(no_sites());
        }
        if !request.site_path.is_empty() {
            if !sites {
                return Err(PanelError::invalid_argument(
                    "a directory of the sites is backed up only with the sites",
                ));
            }
            checked_site_path(&request.site_path)?;
        }
        let configuration = contents.iter().any(|content| {
            matches!(
                content,
                BackupContent::Configuration | BackupContent::Databases
            )
        });
        for (path, content) in &request.attachments {
            if !configuration {
                return Err(PanelError::invalid_argument(
                    "attachments go only with the configuration",
                ));
            }
            attachment_path(path)?;
            if content.len() as u64 > MOST_MEMBER_BYTES {
                return Err(PanelError::resource_exhausted(format!(
                    "{path} is larger than {MOST_MEMBER_BYTES} bytes"
                )));
            }
        }
        let id = Uuid::new_v4();
        let attachments: BTreeMap<&str, String> = request
            .attachments
            .iter()
            .map(|(path, content)| (path.as_str(), hex::encode(content)))
            .collect();
        let names: Vec<&str> = contents.iter().map(|content| content.as_str()).collect();
        let mut transaction = self
            .inner
            .database
            .pool()
            .begin()
            .await
            .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO backups (backup_id, contents, site_path, state, requested_by, \
             requested_at, product_version, attachments) \
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7)",
        )
        .bind(id.to_string())
        .bind(Json(&names))
        .bind(&request.site_path)
        .bind(context.actor())
        .bind(Utc::now())
        .bind(&self.inner.release)
        .bind(Json(&attachments))
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        let mut spec = JobSpec::json(
            JobKind::new(TAKE_JOB)?,
            IdempotencyKey::new(format!("backup-take:{id}"))?,
            &json!({ "backup_id": id }),
            JobOrigin {
                correlation_id: context.correlation_id().clone(),
                causation_id: context.request_id().clone(),
            },
        )?;
        spec.max_attempts = 3;
        self.inner.jobs.enqueue(&spec).await?;
        self.get(&id.to_string()).await
    }

    /// Removes a backup and its archive.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let backup = self.get(id).await?;
        if matches!(backup.state, BackupState::Pending | BackupState::Running) {
            return Err(PanelError::conflict("the backup is still being taken"));
        }
        let archive = self.archive_path(backup.id);
        blocking(move || match fs::remove_file(&archive) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(failure(error)),
            _ => Ok(()),
        })
        .await?;
        sqlx::query("DELETE FROM backups WHERE backup_id = ?1")
            .bind(backup.id.to_string())
            .execute(self.inner.database.pool())
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// A taken backup and its archive.
    pub async fn archive(&self, id: &str) -> Result<(Backup, PathBuf)> {
        let backup = self.get(id).await?;
        match backup.state {
            BackupState::Completed => {
                let path = self.archive_path(backup.id);
                Ok((backup, path))
            }
            BackupState::Failed => Err(PanelError::conflict(
                "the backup failed, so there is no archive",
            )),
            _ => Err(PanelError::conflict("the backup is still being taken")),
        }
    }

    /// The member at `path` of a backup's archive, checked against its
    /// manifest.
    pub async fn member(&self, id: &str, path: &str) -> Result<Vec<u8>> {
        let (_, archive) = self.archive(id).await?;
        let path = path.to_owned();
        blocking(move || panel_backup::read_member(&archive, &path, MOST_MEMBER_BYTES)).await
    }

    /// Replaces `site_path`, a directory below the sites' directory, with
    /// the backup's copy: unpacked beside it, checked, then renamed into
    /// place, so its files are never a mix of both.
    pub async fn restore_sites(&self, id: &str, site_path: &str) -> Result<Extraction> {
        let sites = self.inner.sites.clone().ok_or_else(no_sites)?;
        if site_path.is_empty() {
            return Err(PanelError::invalid_argument(
                "name the directory below the sites' directory to restore",
            ));
        }
        checked_site_path(site_path)?;
        let (backup, archive) = self.archive(id).await?;
        if !backup.contents.contains(&BackupContent::Sites) {
            return Err(PanelError::invalid_argument("the backup holds no sites"));
        }
        let _restoring = self.inner.restoring.lock().await;
        let site_path = site_path.to_owned();
        blocking(move || restore(&archive, &sites, &site_path, backup.id)).await
    }

    /// Takes backup `id`, the work of [`TAKE_JOB`]: once it is pending, or
    /// again when an earlier attempt stopped while running.
    pub async fn take(&self, id: Uuid) -> Result<()> {
        let Some(row) = sqlx::query(
            "UPDATE backups SET state = 'running' \
             WHERE backup_id = ?1 AND state IN ('pending', 'running') \
             RETURNING contents, site_path, attachments",
        )
        .bind(id.to_string())
        .fetch_optional(self.inner.database.pool())
        .await
        .map_err(storage_error)?
        else {
            return Ok(());
        };
        let Json(names): Json<Vec<String>> = row.try_get("contents").map_err(storage_error)?;
        let contents = names
            .iter()
            .map(|name| BackupContent::parse(name).ok_or_else(|| corrupt("contents")))
            .collect::<Result<Vec<_>>>()?;
        let site_path: String = row.try_get("site_path").map_err(storage_error)?;
        let attachments: Option<Json<BTreeMap<String, String>>> =
            row.try_get("attachments").map_err(storage_error)?;
        let attachments = attachments
            .map(|Json(attachments)| attachments)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, content)| {
                hex::decode(content)
                    .map(|content| (path, content))
                    .map_err(|_| corrupt("attachments"))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let staging = self
            .inner
            .directory
            .join(format!("{WORKING_PREFIX}{id}.staging"));
        let taken = self
            .write_archive(id, &contents, &site_path, attachments, &staging)
            .await;
        let leftover = staging.clone();
        let _ = blocking(move || {
            let _ = fs::remove_dir_all(&leftover);
            Ok(())
        })
        .await;
        match taken {
            Ok((files, size, sha256)) => {
                sqlx::query(
                    "UPDATE backups SET state = 'completed', finished_at = ?2, size_bytes = ?3, \
                     sha256 = ?4, files = ?5, attachments = NULL WHERE backup_id = ?1",
                )
                .bind(id.to_string())
                .bind(Utc::now())
                .bind(i64::try_from(size).unwrap_or(i64::MAX))
                .bind(sha256)
                .bind(i64::try_from(files).unwrap_or(i64::MAX))
                .execute(self.inner.database.pool())
                .await
                .map_err(storage_error)?;
                self.prune().await
            }
            Err(error) => {
                tracing::warn!(backup = %id, error_code = %error.code, error = %error.message, "the backup failed");
                sqlx::query(
                    "UPDATE backups SET state = 'failed', finished_at = ?2, failure_code = ?3, \
                     failure_message = ?4, attachments = NULL WHERE backup_id = ?1",
                )
                .bind(id.to_string())
                .bind(Utc::now())
                .bind(error.code.as_str())
                .bind(&error.message)
                .execute(self.inner.database.pool())
                .await
                .map_err(storage_error)?;
                Ok(())
            }
        }
    }

    /// Writes the archive of backup `id`, returning how many files it holds,
    /// its size and its SHA-256.
    async fn write_archive(
        &self,
        id: Uuid,
        contents: &[BackupContent],
        site_path: &str,
        attachments: BTreeMap<String, Vec<u8>>,
        staging: &Path,
    ) -> Result<(u64, u64, String)> {
        let directory = self.inner.directory.clone();
        let prepared = staging.to_owned();
        blocking(move || {
            fs::create_dir_all(&directory).map_err(failure)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .map_err(failure)?;
            }
            match fs::remove_dir_all(&prepared) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(failure(error))
                }
                _ => {}
            }
            fs::create_dir(&prepared).map_err(failure)
        })
        .await?;
        let databases = self.databases(contents)?;
        if !databases.is_empty() {
            let target = staging.join("databases");
            tokio::fs::create_dir(&target).await.map_err(failure)?;
            for name in databases {
                snapshot(
                    &self.inner.data_directory.join(format!("{name}.db")),
                    &target.join(format!("{name}.db")),
                )
                .await?;
            }
        }
        for (path, content) in attachments {
            let target = staging.join(&path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(failure)?;
            }
            tokio::fs::write(target, content).await.map_err(failure)?;
        }
        if contents.contains(&BackupContent::Sites) {
            let sites = self.inner.sites.clone().ok_or_else(no_sites)?;
            let (site_path, target) = (site_path.to_owned(), staging.join("sites"));
            blocking(move || copy_sites(&sites, &site_path, &target)).await?;
        }
        let names: Vec<&'static str> = contents.iter().map(|content| content.as_str()).collect();
        let (staging, release) = (staging.to_owned(), self.inner.release.clone());
        let partial = self
            .inner
            .directory
            .join(format!("{WORKING_PREFIX}{id}.partial"));
        let archive = self.archive_path(id);
        blocking(move || {
            let written = panel_backup::write(&partial, &staging, &release, &names, Utc::now())
                .and_then(|manifest| {
                    fs::rename(&partial, &archive).map_err(failure)?;
                    Ok(manifest)
                });
            let manifest = match written {
                Ok(manifest) => manifest,
                Err(error) => {
                    let _ = fs::remove_file(&partial);
                    return Err(error);
                }
            };
            let (size, sha256) = file_digest(&archive)?;
            Ok((manifest.members.len() as u64, size, sha256))
        })
        .await
    }

    /// The module databases `contents` asks for, by module name.
    fn databases(&self, contents: &[BackupContent]) -> Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for content in contents {
            match content {
                BackupContent::Configuration => {
                    names.insert(CONFIGURATION_DATABASE.to_owned());
                }
                BackupContent::Certificates => {
                    names.insert(crate::MODULE.to_owned());
                }
                BackupContent::Databases => {
                    for entry in fs::read_dir(&self.inner.data_directory).map_err(failure)? {
                        let path = entry.map_err(failure)?.path();
                        if path.extension().is_some_and(|extension| extension == "db") {
                            if let Some(name) = path.file_stem().and_then(OsStr::to_str) {
                                names.insert(name.to_owned());
                            }
                        }
                    }
                }
                BackupContent::Sites => {}
            }
        }
        Ok(names)
    }

    /// Removes finished backups beyond the newest that are kept.
    async fn prune(&self) -> Result<()> {
        let old = sqlx::query(
            "SELECT backup_id FROM backups WHERE state IN ('completed', 'failed') \
             ORDER BY requested_at DESC, backup_id LIMIT -1 OFFSET ?1",
        )
        .bind(i64::try_from(self.inner.kept).unwrap_or(i64::MAX))
        .fetch_all(self.inner.database.pool())
        .await
        .map_err(storage_error)?;
        for row in old {
            let id: String = row.try_get("backup_id").map_err(storage_error)?;
            self.delete(&id).await?;
        }
        Ok(())
    }

    fn archive_path(&self, id: Uuid) -> PathBuf {
        self.inner.directory.join(format!("{id}.tar.zst"))
    }
}

/// Runs [`TAKE_JOB`].
pub struct TakeHandler(pub Backups);

#[async_trait]
impl JobHandler for TakeHandler {
    async fn run(&self, context: JobContext) -> Result<()> {
        #[derive(Deserialize)]
        struct Payload {
            backup_id: Uuid,
        }
        let payload: Payload = context.job().json()?;
        tokio::select! {
            result = self.0.take(payload.backup_id) => result,
            () = context.cancellation().cancelled() => {
                Err(PanelError::unavailable("taking the backup stopped before it finished"))
            }
        }
    }
}

/// Copies the database at `source` to `target` as one consistent snapshot.
async fn snapshot(source: &Path, target: &Path) -> Result<()> {
    if !tokio::fs::try_exists(source).await.unwrap_or(false) {
        return Err(PanelError::not_found(format!(
            "there is no database at {}",
            source.display()
        )));
    }
    let into = target
        .to_str()
        .ok_or_else(|| PanelError::invalid_argument("the data directory's path is not UTF-8"))?;
    let options = SqliteConnectOptions::new()
        .filename(source)
        .create_if_missing(false)
        .busy_timeout(BUSY);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(storage_error)?;
    sqlx::query("VACUUM INTO ?1")
        .bind(into)
        .execute(&mut connection)
        .await
        .map_err(storage_error)?;
    connection.close().await.map_err(storage_error)
}

/// Copies the files and directories of `site_path` below `root`, all of it
/// when empty, to the same place below `target`. Links and the panel's own
/// working entries are left out.
fn copy_sites(root: &Path, site_path: &str, target: &Path) -> Result<()> {
    let (source, destination) = if site_path.is_empty() {
        (root.to_owned(), target.to_owned())
    } else {
        no_links(root, site_path)?;
        (root.join(site_path), target.join(site_path))
    };
    if !source.is_dir() {
        return Err(PanelError::not_found(format!(
            "{site_path} is not a directory of the sites"
        )));
    }
    fs::create_dir_all(&destination).map_err(failure)?;
    for entry in walkdir::WalkDir::new(&source)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !working(entry.file_name()))
    {
        let entry = entry.map_err(|error| failure(error.into()))?;
        let copy = destination.join(
            entry
                .path()
                .strip_prefix(&source)
                .expect("entries are below the source"),
        );
        if entry.file_type().is_dir() {
            fs::create_dir_all(&copy).map_err(failure)?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &copy).map_err(failure)?;
        }
    }
    Ok(())
}

/// Entries the panel makes while it works, and files being written.
fn working(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with(WORKING_PREFIX) || (name.starts_with('.') && name.ends_with(".writing"))
    })
}

fn restore(archive: &Path, root: &Path, site_path: &str, id: Uuid) -> Result<Extraction> {
    let unpacked = root.join(format!("{WORKING_PREFIX}{id}.restoring"));
    let replaced = root.join(format!("{WORKING_PREFIX}{id}.replaced"));
    for leftover in [&unpacked, &replaced] {
        let _ = fs::remove_dir_all(leftover);
    }
    let extraction = panel_backup::extract(archive, &format!("sites/{site_path}"), &unpacked)?;
    let placed = (|| {
        no_links(root, site_path)?;
        let target = root.join(site_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(failure)?;
        }
        let existed = match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => true,
            Ok(_) => {
                return Err(PanelError::conflict(format!(
                    "{site_path} is not a directory"
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(failure(error)),
        };
        if existed {
            fs::rename(&target, &replaced).map_err(failure)?;
        }
        if let Err(error) = fs::rename(&unpacked, &target) {
            if existed {
                let _ = fs::rename(&replaced, &target);
            }
            return Err(failure(error));
        }
        if existed {
            let _ = fs::remove_dir_all(&replaced);
        }
        Ok(())
    })();
    if placed.is_err() {
        let _ = fs::remove_dir_all(&unpacked);
    }
    placed.map(|()| extraction)
}

/// Refuses `relative` below `root` when any part of it is a link, which
/// could lead outside the sites' directory.
fn no_links(root: &Path, relative: &str) -> Result<()> {
    let mut path = root.to_owned();
    for name in relative.split('/') {
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PanelError::invalid_argument(format!(
                    "{relative} passes through a link"
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        }
    }
    Ok(())
}

/// Refuses a directory of the sites that is not `/`-separated names.
fn checked_site_path(path: &str) -> Result<()> {
    let fine = path.len() <= 4096
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains('\\')
                && !name.chars().any(char::is_control)
                && !working(OsStr::new(name))
        });
    if fine {
        Ok(())
    } else {
        Err(PanelError::invalid_argument(format!(
            "{path:?} is not a directory of the sites"
        )))
    }
}

/// Refuses an attachment that is not a JSON file below `configuration/`.
fn attachment_path(path: &str) -> Result<()> {
    let fine = path
        .strip_prefix(ATTACHMENTS)
        .and_then(|name| name.strip_prefix('/'))
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
    if fine {
        Ok(())
    } else {
        Err(PanelError::invalid_argument(format!(
            "attachments are JSON files below {ATTACHMENTS}/, not {path:?}"
        )))
    }
}

fn file_digest(path: &Path) -> Result<(u64, String)> {
    let mut file = fs::File::open(path).map_err(failure)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer).map_err(failure)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

fn backup(row: &SqliteRow) -> Result<Backup> {
    let Json(names): Json<Vec<String>> = row.try_get("contents").map_err(storage_error)?;
    let state: String = row.try_get("state").map_err(storage_error)?;
    let failure_code: Option<String> = row.try_get("failure_code").map_err(storage_error)?;
    let failure_message: Option<String> = row.try_get("failure_message").map_err(storage_error)?;
    let count = |column: &str| -> Result<u64> {
        let value: i64 = row.try_get(column).map_err(storage_error)?;
        u64::try_from(value).map_err(|_| corrupt(column))
    };
    Ok(Backup {
        id: parse_id(
            &row.try_get::<String, _>("backup_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| corrupt("backup ID"))?,
        contents: names
            .iter()
            .map(|name| BackupContent::parse(name).ok_or_else(|| corrupt("contents")))
            .collect::<Result<_>>()?,
        site_path: row.try_get("site_path").map_err(storage_error)?,
        state: BackupState::parse(&state).ok_or_else(|| corrupt("state"))?,
        requested_by: row.try_get("requested_by").map_err(storage_error)?,
        requested_at: row.try_get("requested_at").map_err(storage_error)?,
        finished_at: row.try_get("finished_at").map_err(storage_error)?,
        size_bytes: count("size_bytes")?,
        sha256: row.try_get("sha256").map_err(storage_error)?,
        files: count("files")?,
        failure: failure_code
            .map(|code| PanelError::new(ErrorCode::new(code), failure_message.unwrap_or_default())),
        product_version: row.try_get("product_version").map_err(storage_error)?,
    })
}

fn parse_id(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id)
        .map_err(|_| PanelError::invalid_argument(format!("{id:?} is not a backup ID")))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PanelError::unavailable("the backup work stopped before it finished"))?
}

fn no_sites() -> PanelError {
    PanelError::unsupported_capability(format!(
        "the sites' directory is not available here; set {SITES_ROOT_ENV}"
    ))
}

fn failure(error: io::Error) -> PanelError {
    PanelError::storage_unavailable(format!("cannot read or write backups: {error}"))
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored backup's {what} is unreadable"))
}

#[cfg(test)]
mod tests;
