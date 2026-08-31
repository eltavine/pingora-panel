//! Atomic filesystem adapter for the transport-neutral `SnapshotStore` port.

mod codec;
mod lease;
mod record_file;
mod record_reader;
mod state_directory;

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
use record_file::{open_regular_record, RecordFileOpenError};
use record_reader::{BoundedRecordReadError, BoundedRecordReader, RecordCollectionBudget};
use serde::{de::DeserializeOwned, Serialize};
use state_directory::{StateDirectoryHandle, StateDirectoryOpenError};
use std::{
    ffi::OsStr,
    io::Write,
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

    #[cfg(test)]
    fn prepared_directory(root: &Path) -> PathBuf {
        root.join(PREPARED_DIRECTORY_NAME)
    }

    #[cfg(test)]
    fn prepared_path(root: &Path, token: &PrepareToken) -> PathBuf {
        Self::prepared_directory(root).join(Self::prepared_file_name(token))
    }

    fn prepared_file_name(token: &PrepareToken) -> String {
        let safe_name = ContentHash::from_bytes(token.as_str().as_bytes());
        format!("{}.json", safe_name.as_str())
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
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            let Some(directory) = operation_directory(&root, lease.as_ref(), false)? else {
                return Ok(None);
            };
            let Some(record) = read_record::<ActiveSnapshotRecord>(
                directory.as_ref(),
                OsStr::new(ACTIVE_FILE_NAME),
                codecs.as_ref(),
            )?
            else {
                return Ok(None);
            };
            validate_active_record(&record.value)?;
            Ok(Some(record.value))
        })
        .await
    }

    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            let Some(directory) = operation_directory(&root, lease.as_ref(), false)? else {
                return Ok(Vec::new());
            };
            let Some(prepared) = open_child_directory(
                directory.as_ref(),
                OsStr::new(PREPARED_DIRECTORY_NAME),
                false,
            )?
            else {
                return Ok(Vec::new());
            };
            load_prepared_records(&prepared, usize::MAX, codecs.as_ref())
        })
        .await
    }

    async fn load_prepared_bounded(&self, limit: usize) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            let Some(directory) = operation_directory(&root, lease.as_ref(), false)? else {
                return Ok(Vec::new());
            };
            let Some(prepared) = open_child_directory(
                directory.as_ref(),
                OsStr::new(PREPARED_DIRECTORY_NAME),
                false,
            )?
            else {
                return Ok(Vec::new());
            };
            load_prepared_records(&prepared, limit, codecs.as_ref())
        })
        .await
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            validate_prepared_record(&record)?;
            let directory = operation_directory(&root, lease.as_ref(), true)?
                .expect("creating the state directory returns a handle");
            let prepared = open_child_directory(
                directory.as_ref(),
                OsStr::new(PREPARED_DIRECTORY_NAME),
                true,
            )?
            .expect("creating the prepared directory returns a handle");
            let name = Self::prepared_file_name(&record.receipt.prepare_token);
            atomic_write_record(&prepared, OsStr::new(&name), &record, codecs.as_ref())
        })
        .await
    }

    async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let token = token.clone();
        Self::run_blocking(move || {
            let Some(directory) = operation_directory(&root, lease.as_ref(), false)? else {
                return Ok(());
            };
            let Some(prepared) = open_child_directory(
                directory.as_ref(),
                OsStr::new(PREPARED_DIRECTORY_NAME),
                false,
            )?
            else {
                return Ok(());
            };
            let name = Self::prepared_file_name(&token);
            let path = prepared.path_for(OsStr::new(&name));
            match prepared.remove_file(OsStr::new(&name)) {
                Ok(()) => prepared.sync().map_err(|error| {
                    storage_error("sync prepared directory", prepared.path(), error)
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(storage_error("delete prepared snapshot", &path, error)),
            }
        })
        .await
    }

    async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        Self::run_blocking(move || {
            validate_active_record(&record)?;
            let directory = operation_directory(&root, lease.as_ref(), true)?
                .expect("creating the state directory returns a handle");
            atomic_write_record(
                directory.as_ref(),
                OsStr::new(ACTIVE_FILE_NAME),
                &record,
                codecs.as_ref(),
            )
        })
        .await
    }
}

fn operation_directory(
    root: &Path,
    lease: Option<&Arc<StateDirectoryLease>>,
    create: bool,
) -> Result<Option<Arc<StateDirectoryHandle>>> {
    if let Some(lease) = lease {
        return Ok(Some(lease.directory()));
    }
    StateDirectoryHandle::open_root(root, create)
        .map(|directory| directory.map(Arc::new))
        .map_err(|error| state_directory_open_error("open snapshot directory", root, error))
}

fn open_child_directory(
    parent: &StateDirectoryHandle,
    name: &OsStr,
    create: bool,
) -> Result<Option<StateDirectoryHandle>> {
    parent.open_child_directory(name, create).map_err(|error| {
        state_directory_open_error(
            "open snapshot child directory",
            &parent.path_for(name),
            error,
        )
    })
}

fn load_prepared_records(
    directory: &StateDirectoryHandle,
    limit: usize,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<Vec<PreparedSnapshotRecord>> {
    let entries = directory
        .read_entry_names(MAX_PREPARED_DIRECTORY_ENTRIES.saturating_add(1))
        .map_err(|error| storage_error("read prepared directory", directory.path(), error))?;
    if entries.len() > MAX_PREPARED_DIRECTORY_ENTRIES {
        return Err(PanelError::resource_exhausted(format!(
            "prepared directory exceeds the {MAX_PREPARED_DIRECTORY_ENTRIES} entry scan limit"
        )));
    }
    let mut names = Vec::with_capacity(limit.min(64));
    for name in entries {
        if Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            != Some("json")
        {
            continue;
        }
        if names.len() >= limit {
            return Err(PanelError::resource_exhausted(format!(
                "snapshot store contains more than {limit} prepared records"
            )));
        }
        names.push(name);
    }
    names.sort();

    let mut records = Vec::with_capacity(names.len());
    let mut actual_bytes = RecordCollectionBudget::new(MAX_PREPARED_RECORD_BYTES);
    for name in names {
        let path = directory.path_for(&name);
        let decoded =
            read_record::<PreparedSnapshotRecord>(directory, &name, codecs)?.ok_or_else(|| {
                PanelError::corrupt_state(format!(
                    "prepared snapshot disappeared while loading {}",
                    path.display()
                ))
            })?;
        actual_bytes
            .consume(decoded.encoded_bytes)
            .map_err(|error| {
                PanelError::resource_exhausted(format!(
                    "prepared snapshot records exceed the {} byte aggregate limit",
                    error.max_bytes
                ))
            })?;
        let record = decoded.value;
        validate_prepared_record(&record)?;
        let expected = FileSnapshotStore::prepared_file_name(&record.receipt.prepare_token);
        if name != OsStr::new(&expected) {
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

struct DecodedRecord<T> {
    value: T,
    encoded_bytes: u64,
}

fn read_record<T: DeserializeOwned>(
    directory: &StateDirectoryHandle,
    name: &OsStr,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<Option<DecodedRecord<T>>> {
    let path = directory.path_for(name);
    let opened = match open_regular_record(directory, name) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Ok(None),
        Err(RecordFileOpenError::NotRegular) => {
            return Err(PanelError::corrupt_state(format!(
                "snapshot record is not a regular file: {}",
                path.display()
            )));
        }
        Err(RecordFileOpenError::Io(error)) => {
            return Err(storage_error("open snapshot record", &path, error));
        }
    };
    if opened.length_hint > MAX_RECORD_BYTES {
        return Err(PanelError::corrupt_state(format!(
            "snapshot record exceeds the {} byte limit: {}",
            MAX_RECORD_BYTES,
            path.display()
        )));
    }

    let bytes =
        match BoundedRecordReader::new(MAX_RECORD_BYTES).read(opened.file, opened.length_hint) {
            Ok(bytes) => bytes,
            Err(BoundedRecordReadError::Io(error)) => {
                return Err(storage_error("read snapshot record", &path, error));
            }
            Err(BoundedRecordReadError::LimitExceeded { max_bytes }) => {
                return Err(PanelError::corrupt_state(format!(
                    "snapshot record exceeds the {max_bytes} byte limit: {}",
                    path.display()
                )));
            }
        };
    let encoded_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    codecs.decode(&bytes).map(|value| {
        Some(DecodedRecord {
            value,
            encoded_bytes,
        })
    })
}

fn atomic_write_record<T: Serialize>(
    directory: &StateDirectoryHandle,
    name: &OsStr,
    payload: &T,
    codecs: &SnapshotRecordCodecRegistry,
) -> Result<()> {
    let path = directory.path_for(name);
    let bytes = codecs.encode(payload)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(PanelError::invalid_argument(format!(
            "snapshot record exceeds the {MAX_RECORD_BYTES} byte limit"
        )));
    }

    let temporary_id = Uuid::new_v4();
    let temporary_name = format!(".snapshot-{temporary_id}.tmp");
    let temporary = directory.path_for(OsStr::new(&temporary_name));
    let write_result = (|| {
        let mut file = directory
            .create_new_file(OsStr::new(&temporary_name))
            .map_err(|error| storage_error("create temporary snapshot", &temporary, error))?;
        file.write_all(&bytes)
            .map_err(|error| storage_error("write temporary snapshot", &temporary, error))?;
        file.sync_all()
            .map_err(|error| storage_error("sync temporary snapshot", &temporary, error))?;
        directory
            .rename_file(OsStr::new(&temporary_name), name)
            .map_err(|error| storage_error("activate snapshot record", &path, error))?;
        directory
            .sync()
            .map_err(|error| storage_error("sync snapshot directory", directory.path(), error))
    })();

    if write_result.is_err() {
        let _ = directory.remove_file(OsStr::new(&temporary_name));
    }
    write_result
}

fn state_directory_open_error(
    operation: &str,
    path: &Path,
    error: StateDirectoryOpenError,
) -> PanelError {
    match error {
        StateDirectoryOpenError::NotDirectory => PanelError::corrupt_state(format!(
            "snapshot directory is not a regular directory: {}",
            path.display()
        )),
        StateDirectoryOpenError::Io(error) => storage_error(operation, path, error),
    }
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
    use std::fs;

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

    #[cfg(unix)]
    fn active_record(revision: u64) -> ActiveSnapshotRecord {
        let prepared = prepared_record(revision);
        ActiveSnapshotRecord {
            envelope: prepared.envelope,
            receipt: ActivationReceipt {
                revision_id: prepared.receipt.revision_id,
                content_hash: prepared.receipt.content_hash,
                adapter_version: prepared.receipt.adapter_version,
                schema_version: prepared.receipt.schema_version,
                prepare_token: prepared.receipt.prepare_token,
                previous_active_hash: None,
            },
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
    async fn active_record_symlink_is_rejected_without_following_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = TemporaryDirectory::new();
        fs::create_dir_all(&temporary.0).unwrap();
        let target_path = temporary.0.join("outside-record.json");
        let target_contents = b"target must remain untouched";
        fs::write(&target_path, target_contents).unwrap();
        symlink(&target_path, temporary.0.join(ACTIVE_FILE_NAME)).unwrap();
        let store = FileSnapshotStore::new(&temporary.0);

        let error = store.load_active().await.unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::CORRUPT_STATE);
        assert_eq!(fs::read(target_path).unwrap(), target_contents);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn active_record_fifo_is_rejected_without_blocking_the_worker() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, time::Duration};

        let temporary = TemporaryDirectory::new();
        fs::create_dir_all(&temporary.0).unwrap();
        let active_path = temporary.0.join(ACTIVE_FILE_NAME);
        let active_path = CString::new(active_path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is a valid, NUL-terminated C string and the mode is
        // restricted to the owning test process.
        assert_eq!(unsafe { libc::mkfifo(active_path.as_ptr(), 0o600) }, 0);
        let store = FileSnapshotStore::new(&temporary.0);

        let error = tokio::time::timeout(Duration::from_millis(200), store.load_active())
            .await
            .expect("opening a FIFO must remain non-blocking")
            .unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::CORRUPT_STATE);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn exclusive_store_remains_anchored_when_its_root_path_is_replaced() {
        let temporary = TemporaryDirectory::new();
        fs::create_dir_all(&temporary.0).unwrap();
        let configured_root = temporary.0.join("state");
        let anchored_root = temporary.0.join("renamed-state");
        let store = FileSnapshotStore::open_exclusive(&configured_root)
            .await
            .unwrap();
        fs::rename(&configured_root, &anchored_root).unwrap();
        fs::create_dir(&configured_root).unwrap();
        let active = active_record(7);

        store.commit_activation(active.clone()).await.unwrap();

        assert_eq!(store.load_active().await.unwrap(), Some(active));
        assert!(anchored_root.join(ACTIVE_FILE_NAME).is_file());
        assert!(!configured_root.join(ACTIVE_FILE_NAME).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_scan_remains_anchored_when_its_root_path_is_replaced() {
        let temporary = TemporaryDirectory::new();
        fs::create_dir_all(&temporary.0).unwrap();
        let configured_root = temporary.0.join("state");
        let anchored_root = temporary.0.join("renamed-state");
        let store = FileSnapshotStore::open_exclusive(&configured_root)
            .await
            .unwrap();
        let prepared = prepared_record(9);
        store.save_prepared(prepared.clone()).await.unwrap();
        fs::rename(&configured_root, &anchored_root).unwrap();
        fs::create_dir(&configured_root).unwrap();

        assert_eq!(store.load_prepared().await.unwrap(), vec![prepared]);
        assert!(anchored_root.join(PREPARED_DIRECTORY_NAME).is_dir());
        assert!(!configured_root.join(PREPARED_DIRECTORY_NAME).exists());
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
