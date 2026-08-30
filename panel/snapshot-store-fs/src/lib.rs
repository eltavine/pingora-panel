//! Atomic filesystem adapter for the transport-neutral `SnapshotStore` port.

mod codec;
mod lease;

pub use codec::{
    JsonSnapshotRecordCodecV1, SnapshotRecordCodec, SnapshotRecordCodecRegistry,
    JSON_SNAPSHOT_RECORD_FORMAT_V1,
};
pub use lease::StateDirectoryLease;

use async_trait::async_trait;
use panel_domain::ContentHash;
use panel_engine::{ActiveSnapshotRecord, PrepareToken, PreparedSnapshotRecord, SnapshotStore};
#[cfg(test)]
use panel_errors::ErrorCode;
use panel_errors::{PanelError, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const ACTIVE_FILE_NAME: &str = "active.json";
const PREPARED_DIRECTORY_NAME: &str = "prepared";
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREPARED_RECORD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PREPARED_DIRECTORY_ENTRIES: usize = 4096;

#[derive(Clone, Debug)]
pub struct FileSnapshotStore {
    root: PathBuf,
    lease: Option<Arc<StateDirectoryLease>>,
    codecs: Arc<SnapshotRecordCodecRegistry>,
}

impl FileSnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lease: None,
            codecs: Arc::new(SnapshotRecordCodecRegistry::default()),
        }
    }

    pub fn with_codec_registry(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
    ) -> Self {
        Self {
            root: root.into(),
            lease: None,
            codecs,
        }
    }

    /// Open a store with process-lifetime exclusive ownership of its directory.
    ///
    /// `new` remains available for offline tooling and backwards compatibility;
    /// long-running gateway compositions should use this constructor.
    pub async fn open_exclusive(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_exclusive_with_codec_registry(
            root,
            Arc::new(SnapshotRecordCodecRegistry::default()),
        )
        .await
    }

    pub async fn open_exclusive_with_codec_registry(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
    ) -> Result<Self> {
        let root = root.into();
        let lease = Arc::new(StateDirectoryLease::acquire(root.clone()).await?);
        Ok(Self {
            root,
            lease: Some(lease),
            codecs,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has_exclusive_lease(&self) -> bool {
        self.lease.is_some()
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
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            let path = Self::active_path(&root);
            let Some(record) = read_record::<ActiveSnapshotRecord>(&path, codecs.as_ref())? else {
                return Ok(None);
            };
            validate_active_record(&record)?;
            Ok(Some(record))
        })
        .await
    }

    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || load_prepared_records(&root, usize::MAX, codecs.as_ref())).await
    }

    async fn load_prepared_bounded(&self, limit: usize) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || load_prepared_records(&root, limit, codecs.as_ref())).await
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            validate_prepared_record(&record)?;
            let path = Self::prepared_path(&root, &record.receipt.prepare_token);
            atomic_write_record(&path, &record, codecs.as_ref())
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
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            validate_active_record(&record)?;
            atomic_write_record(&Self::active_path(&root), &record, codecs.as_ref())
        })
        .await
    }
}

fn load_prepared_records(
    root: &Path,
    limit: usize,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<Vec<PreparedSnapshotRecord>> {
    let directory = FileSnapshotStore::prepared_directory(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    ensure_directory(&directory)?;
    let entries = fs::read_dir(&directory)
        .map_err(|error| storage_error("read prepared directory", &directory, error))?;
    let mut paths = Vec::with_capacity(limit.min(64));
    for (index, entry) in entries.enumerate() {
        if index >= MAX_PREPARED_DIRECTORY_ENTRIES {
            return Err(PanelError::resource_exhausted(format!(
                "prepared directory exceeds the {MAX_PREPARED_DIRECTORY_ENTRIES} entry scan limit"
            )));
        }
        let path = entry
            .map_err(|error| storage_error("read prepared entry", &directory, error))?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if paths.len() >= limit {
            return Err(PanelError::resource_exhausted(format!(
                "snapshot store contains more than {limit} prepared records"
            )));
        }
        paths.push(path);
    }
    let total_bytes = paths.iter().try_fold(0_u64, |total, path| {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| storage_error("read prepared snapshot metadata", path, error))?;
        total.checked_add(metadata.len()).ok_or_else(|| {
            PanelError::resource_exhausted("prepared snapshot aggregate size overflow")
        })
    })?;
    if total_bytes > MAX_PREPARED_RECORD_BYTES {
        return Err(PanelError::resource_exhausted(format!(
            "prepared snapshot records exceed the {MAX_PREPARED_RECORD_BYTES} byte aggregate limit"
        )));
    }
    paths.sort();

    let mut records = Vec::with_capacity(paths.len());
    for path in paths {
        let record = read_record::<PreparedSnapshotRecord>(&path, codecs)?.ok_or_else(|| {
            PanelError::corrupt_state(format!(
                "prepared snapshot disappeared while loading {}",
                path.display()
            ))
        })?;
        validate_prepared_record(&record)?;
        let expected = FileSnapshotStore::prepared_path(root, &record.receipt.prepare_token);
        if path != expected {
            return Err(PanelError::corrupt_state(format!(
                "prepared snapshot filename does not match its token: {}",
                path.display()
            )));
        }
        records.push(record);
    }
    Ok(records)
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

fn read_record<T: DeserializeOwned>(
    path: &Path,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<Option<T>> {
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
    codecs.decode(&bytes).map(Some)
}

fn atomic_write_record<T: Serialize>(
    path: &Path,
    payload: &T,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| PanelError::invalid_argument("snapshot path has no parent directory"))?;
    ensure_directory(parent)?;
    let bytes = codecs.encode(payload)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(PanelError::invalid_argument(format!(
            "snapshot record exceeds the {MAX_RECORD_BYTES} byte limit"
        )));
    }

    let temporary_id = Uuid::new_v4();
    let temporary = parent.join(format!(".snapshot-{temporary_id}.tmp"));
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

    struct JsonSnapshotRecordCodecV2;

    impl SnapshotRecordCodec for JsonSnapshotRecordCodecV2 {
        fn format_version(&self) -> u32 {
            2
        }

        fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
            let mut record = br#"{"format_version":2,"payload":"#.to_vec();
            record.extend_from_slice(payload);
            record.push(b'}');
            Ok(record)
        }

        fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>> {
            let record: serde_json::Value = serde_json::from_slice(record).unwrap();
            Ok(serde_json::to_vec(&record["payload"]).unwrap())
        }
    }

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
    async fn injected_codec_registry_changes_format_without_store_changes() {
        let temporary = TemporaryDirectory::new();
        let codecs = SnapshotRecordCodecRegistry::new(Arc::new(JsonSnapshotRecordCodecV2))
            .unwrap()
            .with_reader(Arc::new(JsonSnapshotRecordCodecV1))
            .unwrap();
        let store = FileSnapshotStore::with_codec_registry(&temporary.0, Arc::new(codecs));
        let prepared = prepared_record(1);

        store.save_prepared(prepared.clone()).await.unwrap();

        let path = FileSnapshotStore::prepared_path(&temporary.0, &prepared.receipt.prepare_token);
        let bytes = fs::read(path).unwrap();
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains(r#""format_version":2"#));
        assert_eq!(store.load_prepared().await.unwrap(), [prepared]);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn exclusive_store_rejects_a_second_owner_until_release() {
        let temporary = TemporaryDirectory::new();
        let first = FileSnapshotStore::open_exclusive(&temporary.0)
            .await
            .unwrap();
        assert!(first.has_exclusive_lease());

        let error = FileSnapshotStore::open_exclusive(&temporary.0)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::PRECONDITION_FAILED);

        drop(first);
        assert!(FileSnapshotStore::open_exclusive(&temporary.0)
            .await
            .unwrap()
            .has_exclusive_lease());
    }

    #[tokio::test]
    async fn bounded_load_rejects_before_materializing_excess_records() {
        let temporary = TemporaryDirectory::new();
        let store = FileSnapshotStore::new(&temporary.0);
        store.save_prepared(prepared_record(1)).await.unwrap();
        store.save_prepared(prepared_record(2)).await.unwrap();

        let error = store.load_prepared_bounded(1).await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
    }

    #[tokio::test]
    async fn prepared_directory_scan_has_an_independent_entry_ceiling() {
        let temporary = TemporaryDirectory::new();
        let directory = FileSnapshotStore::prepared_directory(&temporary.0);
        fs::create_dir_all(&directory).unwrap();
        for index in 0..=MAX_PREPARED_DIRECTORY_ENTRIES {
            fs::write(directory.join(format!("ignored-{index}.tmp")), []).unwrap();
        }

        let store = FileSnapshotStore::new(&temporary.0);
        let error = store
            .load_prepared_bounded(MAX_PREPARED_DIRECTORY_ENTRIES)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
    }
}
