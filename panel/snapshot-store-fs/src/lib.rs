//! Atomic filesystem adapter for the transport-neutral `SnapshotStore` port.

use async_trait::async_trait;
use panel_domain::ContentHash;
use panel_engine::{ActiveSnapshotRecord, PrepareToken, PreparedSnapshotRecord, SnapshotStore};
use panel_errors::{ErrorCode, PanelError, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const STORE_FORMAT_VERSION: u32 = 1;
const ACTIVE_FILE_NAME: &str = "active.json";
const PREPARED_DIRECTORY_NAME: &str = "prepared";
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct FileSnapshotStore {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct DiskRecord<T> {
    format_version: u32,
    payload: T,
}

impl FileSnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn active_path(root: &Path) -> PathBuf {
        root.join(ACTIVE_FILE_NAME)
    }

    fn prepared_directory(root: &Path) -> PathBuf {
        root.join(PREPARED_DIRECTORY_NAME)
    }

    fn prepared_path(root: &Path, token: &PrepareToken) -> PathBuf {
        let safe_name = ContentHash::from_bytes(token.as_str().as_bytes());
        Self::prepared_directory(root).join(format!("{}.json", safe_name.as_str()))
    }

    async fn run_blocking<T>(operation: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
    where
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(operation)
            .await
            .map_err(|error| {
                PanelError::internal("snapshot store worker failed").with_source(error)
            })?
    }
}

#[async_trait]
impl SnapshotStore for FileSnapshotStore {
    async fn load_active(&self) -> Result<Option<ActiveSnapshotRecord>> {
        let root = self.root.clone();
        Self::run_blocking(move || {
            let path = Self::active_path(&root);
            let Some(record) = read_record::<ActiveSnapshotRecord>(&path)? else {
                return Ok(None);
            };
            validate_active_record(&record)?;
            Ok(Some(record))
        })
        .await
    }

    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        Self::run_blocking(move || {
            let directory = Self::prepared_directory(&root);
            if !directory.exists() {
                return Ok(Vec::new());
            }
            ensure_directory(&directory)?;
            let mut paths = fs::read_dir(&directory)
                .map_err(|error| storage_error("read prepared directory", &directory, error))?
                .map(|entry| {
                    entry
                        .map(|value| value.path())
                        .map_err(|error| storage_error("read prepared entry", &directory, error))
                })
                .collect::<Result<Vec<_>>>()?;
            paths.sort();

            let mut records = Vec::new();
            for path in paths {
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let record = read_record::<PreparedSnapshotRecord>(&path)?.ok_or_else(|| {
                    PanelError::corrupt_state(format!(
                        "prepared snapshot disappeared while loading {}",
                        path.display()
                    ))
                })?;
                validate_prepared_record(&record)?;
                let expected = Self::prepared_path(&root, &record.receipt.prepare_token);
                if path != expected {
                    return Err(PanelError::corrupt_state(format!(
                        "prepared snapshot filename does not match its token: {}",
                        path.display()
                    )));
                }
                records.push(record);
            }
            Ok(records)
        })
        .await
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        Self::run_blocking(move || {
            validate_prepared_record(&record)?;
            let path = Self::prepared_path(&root, &record.receipt.prepare_token);
            atomic_write_record(&path, &record)
        })
        .await
    }

    async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
        let root = self.root.clone();
        let token = token.clone();
        Self::run_blocking(move || {
            let path = Self::prepared_path(&root, &token);
            match fs::remove_file(&path) {
                Ok(()) => sync_directory(path.parent().expect("prepared path has a parent")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(storage_error("delete prepared snapshot", &path, error)),
            }
        })
        .await
    }

    async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        Self::run_blocking(move || {
            validate_active_record(&record)?;
            atomic_write_record(&Self::active_path(&root), &record)
        })
        .await
    }
}

fn validate_prepared_record(record: &PreparedSnapshotRecord) -> Result<()> {
    let snapshot = &record.envelope.snapshot;
    if !snapshot.has_valid_content_hash()
        || record.receipt.revision_id != snapshot.revision_id
        || record.receipt.content_hash != snapshot.content_hash
        || record.receipt.schema_version != snapshot.schema_version
    {
        return Err(PanelError::corrupt_state(
            "prepared snapshot metadata does not match its immutable payload",
        ));
    }
    Ok(())
}

fn validate_active_record(record: &ActiveSnapshotRecord) -> Result<()> {
    let snapshot = &record.envelope.snapshot;
    if !snapshot.has_valid_content_hash()
        || record.receipt.revision_id != snapshot.revision_id
        || record.receipt.content_hash != snapshot.content_hash
        || record.receipt.schema_version != snapshot.schema_version
    {
        return Err(PanelError::corrupt_state(
            "active snapshot metadata does not match its immutable payload",
        ));
    }
    Ok(())
}

fn read_record<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage_error("read snapshot metadata", path, error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PanelError::corrupt_state(format!(
            "snapshot record is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(PanelError::corrupt_state(format!(
            "snapshot record exceeds the {} byte limit: {}",
            MAX_RECORD_BYTES,
            path.display()
        )));
    }

    let mut file =
        File::open(path).map_err(|error| storage_error("open snapshot record", path, error))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| storage_error("read snapshot record", path, error))?;
    let disk: DiskRecord<T> = serde_json::from_slice(&bytes).map_err(|error| {
        PanelError::corrupt_state(format!("invalid snapshot record at {}", path.display()))
            .with_source(error)
    })?;
    if disk.format_version != STORE_FORMAT_VERSION {
        return Err(PanelError::new(
            ErrorCode::UNSUPPORTED_CAPABILITY,
            format!(
                "snapshot store format {} is not supported (expected {})",
                disk.format_version, STORE_FORMAT_VERSION
            ),
        ));
    }
    Ok(Some(disk.payload))
}

fn atomic_write_record<T: Serialize>(path: &Path, payload: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| PanelError::invalid_argument("snapshot path has no parent directory"))?;
    ensure_directory(parent)?;
    let bytes = serde_json::to_vec(&DiskRecord {
        format_version: STORE_FORMAT_VERSION,
        payload,
    })
    .map_err(|error| PanelError::internal("serialize snapshot record").with_source(error))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(PanelError::invalid_argument(format!(
            "snapshot record exceeds the {} byte limit",
            MAX_RECORD_BYTES
        )));
    }

    let temporary = parent.join(format!(".snapshot-{}.tmp", Uuid::new_v4()));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| storage_error("create temporary snapshot", &temporary, error))?;
        file.write_all(&bytes)
            .map_err(|error| storage_error("write temporary snapshot", &temporary, error))?;
        file.sync_all()
            .map_err(|error| storage_error("sync temporary snapshot", &temporary, error))?;
        fs::rename(&temporary, path)
            .map_err(|error| storage_error("activate snapshot record", path, error))?;
        sync_directory(parent)
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn sync_directory(path: &Path) -> Result<()> {
    let directory =
        File::open(path).map_err(|error| storage_error("open snapshot directory", path, error))?;
    directory
        .sync_all()
        .map_err(|error| storage_error("sync snapshot directory", path, error))
}

fn ensure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|error| storage_error("create snapshot directory", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| storage_error("read snapshot directory metadata", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PanelError::corrupt_state(format!(
            "snapshot directory is not a regular directory: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| storage_error("secure snapshot directory", path, error))?;
    }
    Ok(())
}

fn storage_error(operation: &str, path: &Path, error: std::io::Error) -> PanelError {
    PanelError::storage_unavailable(format!("{operation}: {}", path.display())).with_source(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::RevisionId;
    use panel_engine::{
        ActivationReceipt, PrepareReceipt, PreparedSnapshotRecord, SnapshotEnvelope,
    };
    use panel_ir::RuntimeSnapshot;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pingora-panel-{}", Uuid::new_v4()));
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn prepared_record(revision: u64) -> PreparedSnapshotRecord {
        let snapshot = RuntimeSnapshot::empty(RevisionId::new(revision));
        let token = PrepareToken::new(format!("prepare-{revision}"));
        PreparedSnapshotRecord {
            receipt: PrepareReceipt {
                revision_id: snapshot.revision_id,
                content_hash: snapshot.content_hash.clone(),
                adapter_version: "test".into(),
                schema_version: snapshot.schema_version.clone(),
                prepare_token: token,
                previous_active_hash: None,
            },
            envelope: SnapshotEnvelope { snapshot },
        }
    }

    #[tokio::test]
    async fn prepared_and_active_records_round_trip() {
        let temporary = TemporaryDirectory::new();
        let store = FileSnapshotStore::new(&temporary.0);
        let prepared = prepared_record(1);
        store.save_prepared(prepared.clone()).await.unwrap();
        assert_eq!(store.load_prepared().await.unwrap(), vec![prepared.clone()]);

        let active = ActiveSnapshotRecord {
            envelope: prepared.envelope.clone(),
            receipt: ActivationReceipt {
                revision_id: prepared.receipt.revision_id,
                content_hash: prepared.receipt.content_hash.clone(),
                adapter_version: prepared.receipt.adapter_version.clone(),
                schema_version: prepared.receipt.schema_version.clone(),
                prepare_token: prepared.receipt.prepare_token.clone(),
                previous_active_hash: None,
            },
        };
        store.commit_activation(active.clone()).await.unwrap();
        assert_eq!(store.load_active().await.unwrap(), Some(active));

        store
            .delete_prepared(&prepared.receipt.prepare_token)
            .await
            .unwrap();
        assert!(store.load_prepared().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn corrupt_active_record_is_rejected_without_deletion() {
        let temporary = TemporaryDirectory::new();
        fs::create_dir_all(&temporary.0).unwrap();
        let active_path = temporary.0.join(ACTIVE_FILE_NAME);
        fs::write(&active_path, b"not-json").unwrap();
        let store = FileSnapshotStore::new(&temporary.0);

        let error = store.load_active().await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::CORRUPT_STATE);
        assert!(active_path.exists());
    }
}
