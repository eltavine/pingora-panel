#![forbid(clippy::undocumented_unsafe_blocks)]
#![forbid(unsafe_op_in_unsafe_fn)]

//! Atomic filesystem adapter for the transport-neutral `SnapshotStore` port.

mod atomic_file;
mod codec;
mod lease;
mod limits;
mod record_file;
mod record_reader;
mod state_directory;

pub use codec::{
    JsonSnapshotRecordCodecV1, SnapshotRecordCodec, SnapshotRecordCodecRegistry,
    JSON_SNAPSHOT_RECORD_FORMAT_V1,
};
pub use lease::StateDirectoryLease;
pub use limits::{
    SnapshotStoreLimits, SnapshotStoreLimitsError, DEFAULT_MAX_PREPARED_DIRECTORY_ENTRIES,
    DEFAULT_MAX_PREPARED_RECORD_BYTES, DEFAULT_MAX_RECORD_BYTES,
};

use async_trait::async_trait;
use atomic_file::{AtomicFilePublisher, AtomicPublishError, AtomicPublishStage, TemporaryPrefix};
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
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[cfg(test)]
use uuid::Uuid;

const ACTIVE_FILE_NAME: &str = "active.json";
const PREPARED_DIRECTORY_NAME: &str = "prepared";

/// Namespace for this store's in-flight snapshot writes.
///
/// Declared once so publication and crash reclamation cannot disagree.
const SNAPSHOT_TEMPORARY_PREFIX: TemporaryPrefix = TemporaryPrefix::new(".snapshot-");
#[derive(Clone, Debug)]
pub struct FileSnapshotStore {
    root: PathBuf,
    lease: Option<Arc<StateDirectoryLease>>,
    codecs: Arc<SnapshotRecordCodecRegistry>,
    limits: SnapshotStoreLimits,
    operation_gate: Arc<Mutex<()>>,
}

impl FileSnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lease: None,
            codecs: Arc::new(SnapshotRecordCodecRegistry::default()),
            limits: SnapshotStoreLimits::default(),
            operation_gate: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_limits(root: impl Into<PathBuf>, limits: SnapshotStoreLimits) -> Self {
        Self {
            root: root.into(),
            lease: None,
            codecs: Arc::new(SnapshotRecordCodecRegistry::default()),
            limits,
            operation_gate: Arc::new(Mutex::new(())),
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
            limits: SnapshotStoreLimits::default(),
            operation_gate: Arc::new(Mutex::new(())),
        }
    }

    /// Creates a store, rejecting a limit set whose ceilings contradict.
    ///
    /// The infallible [`Self::with_limits`] stays for callers composing from a
    /// limit set they have already validated, so adding this costs them nothing.
    pub fn try_with_limits(root: impl Into<PathBuf>, limits: SnapshotStoreLimits) -> Result<Self> {
        Self::try_with_codec_registry_and_limits(
            root,
            Arc::new(SnapshotRecordCodecRegistry::default()),
            limits,
        )
    }

    /// Creates a store with an injected codec registry, rejecting a
    /// contradictory limit set.
    pub fn try_with_codec_registry_and_limits(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
        limits: SnapshotStoreLimits,
    ) -> Result<Self> {
        usable_limits(limits)?;
        Ok(Self::with_codec_registry_and_limits(root, codecs, limits))
    }

    pub fn with_codec_registry_and_limits(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
        limits: SnapshotStoreLimits,
    ) -> Self {
        Self {
            root: root.into(),
            lease: None,
            codecs,
            limits,
            operation_gate: Arc::new(Mutex::new(())),
        }
    }

    /// Open a store with process-lifetime exclusive ownership of its directory.
    ///
    /// `new` remains available for offline tooling and backwards compatibility;
    /// long-running gateway compositions should use this constructor.
    pub async fn open_exclusive(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_exclusive_with_codec_registry_and_limits(
            root,
            Arc::new(SnapshotRecordCodecRegistry::default()),
            SnapshotStoreLimits::default(),
        )
        .await
    }

    pub async fn open_exclusive_with_limits(
        root: impl Into<PathBuf>,
        limits: SnapshotStoreLimits,
    ) -> Result<Self> {
        Self::open_exclusive_with_codec_registry_and_limits(
            root,
            Arc::new(SnapshotRecordCodecRegistry::default()),
            limits,
        )
        .await
    }

    pub async fn open_exclusive_with_codec_registry(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
    ) -> Result<Self> {
        Self::open_exclusive_with_codec_registry_and_limits(
            root,
            codecs,
            SnapshotStoreLimits::default(),
        )
        .await
    }

    pub async fn open_exclusive_with_codec_registry_and_limits(
        root: impl Into<PathBuf>,
        codecs: Arc<SnapshotRecordCodecRegistry>,
        limits: SnapshotStoreLimits,
    ) -> Result<Self> {
        usable_limits(limits)?;
        let root = root.into();
        let lease = Arc::new(StateDirectoryLease::acquire(root.clone()).await?);
        // The scan is bounded by the same ceiling that bounds reading prepared
        // records, so a directory stuffed with entries cannot make opening
        // consume unbounded memory. Orphans dominate any oversized directory, so
        // a bounded pass still drains them across successive opens.
        Self::reclaim_abandoned_temporaries(
            Arc::clone(&lease),
            limits.max_prepared_directory_entries(),
        )
        .await?;
        Ok(Self {
            root,
            lease: Some(lease),
            codecs,
            limits,
            operation_gate: Arc::new(Mutex::new(())),
        })
    }

    /// Removes snapshot temporaries left behind by a process that died mid-write.
    ///
    /// A temporary belonging to a live writer is indistinguishable from an
    /// abandoned one, so this may only run while no other writer exists. The
    /// lease is taken as an argument rather than read from `self` so that the
    /// compiler enforces it: without holding the lease there is nothing to pass.
    ///
    /// Reclamation runs once at open rather than on every write, so the steady
    /// state costs nothing.
    async fn reclaim_abandoned_temporaries(
        lease: Arc<StateDirectoryLease>,
        max_entries: usize,
    ) -> Result<()> {
        Self::run_blocking(move || {
            let directory = lease.directory();
            reclaim_temporaries(directory.as_ref(), max_entries)?;
            if let Some(prepared) = open_child_directory(
                directory.as_ref(),
                OsStr::new(PREPARED_DIRECTORY_NAME),
                false,
            )? {
                reclaim_temporaries(&prepared, max_entries)?;
            }
            Ok(())
        })
        .await
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has_exclusive_lease(&self) -> bool {
        self.lease.is_some()
    }

    pub fn limits(&self) -> SnapshotStoreLimits {
        self.limits
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
        let limits = self.limits;
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
            let Some(directory) = operation_directory(&root, lease.as_ref(), false)? else {
                return Ok(None);
            };
            let Some(record) = read_record::<ActiveSnapshotRecord>(
                directory.as_ref(),
                OsStr::new(ACTIVE_FILE_NAME),
                codecs.as_ref(),
                limits.max_record_bytes(),
                None,
            )?
            else {
                return Ok(None);
            };
            validate_active_record(&record)?;
            Ok(Some(record))
        })
        .await
    }

    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        let limits = self.limits;
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
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
            load_prepared_records(&prepared, usize::MAX, codecs.as_ref(), limits)
        })
        .await
    }

    async fn load_prepared_bounded(&self, limit: usize) -> Result<Vec<PreparedSnapshotRecord>> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        let limits = self.limits;
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
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
            load_prepared_records(&prepared, limit, codecs.as_ref(), limits)
        })
        .await
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let codecs = Arc::clone(&self.codecs);
        let limits = self.limits;
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
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
            let name = OsStr::new(&name);
            let bytes = encode_record(&record, codecs.as_ref(), limits.max_record_bytes())?;
            ensure_prepared_write_capacity(&prepared, name, bytes.len(), limits)?;
            publish_record(&prepared, name, &bytes)
        })
        .await
    }

    async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
        let root = self.root.clone();
        let lease = self.lease.clone();
        let token = token.clone();
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
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
        let limits = self.limits;
        let operation_gate = Arc::clone(&self.operation_gate);
        Self::run_blocking(move || {
            let _operation = acquire_operation_gate(operation_gate.as_ref())?;
            validate_active_record(&record)?;
            let directory = operation_directory(&root, lease.as_ref(), true)?
                .expect("creating the state directory returns a handle");
            atomic_write_record(
                directory.as_ref(),
                OsStr::new(ACTIVE_FILE_NAME),
                &record,
                codecs.as_ref(),
                limits.max_record_bytes(),
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

fn acquire_operation_gate(gate: &Mutex<()>) -> Result<std::sync::MutexGuard<'_, ()>> {
    gate.lock()
        .map_err(|_| PanelError::internal("snapshot store operation gate is poisoned"))
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
    limits: SnapshotStoreLimits,
) -> Result<Vec<PreparedSnapshotRecord>> {
    let max_entries = limits.max_prepared_directory_entries();
    let entries = directory
        .read_entry_names(max_entries.saturating_add(1))
        .map_err(|error| storage_error("read prepared directory", directory.path(), error))?;
    if entries.len() > max_entries {
        return Err(PanelError::resource_exhausted(format!(
            "prepared directory exceeds the {max_entries} entry scan limit"
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
    let mut actual_bytes = RecordCollectionBudget::new(limits.max_prepared_record_bytes());
    for name in names {
        let path = directory.path_for(&name);
        let decoded = read_record::<PreparedSnapshotRecord>(
            directory,
            &name,
            codecs,
            limits.max_record_bytes(),
            Some(&mut actual_bytes),
        )?
        .ok_or_else(|| {
            PanelError::corrupt_state(format!(
                "prepared snapshot disappeared while loading {}",
                path.display()
            ))
        })?;
        let record = decoded;
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

fn read_record<T: DeserializeOwned>(
    directory: &StateDirectoryHandle,
    name: &OsStr,
    codecs: &SnapshotRecordCodecRegistry,
    max_record_bytes: u64,
    collection_budget: Option<&mut RecordCollectionBudget>,
) -> Result<Option<T>> {
    let Some(bytes) = read_record_bytes(directory, name, max_record_bytes, collection_budget)?
    else {
        return Ok(None);
    };
    codecs.decode(&bytes).map(Some)
}

fn read_record_bytes(
    directory: &StateDirectoryHandle,
    name: &OsStr,
    max_record_bytes: u64,
    collection_budget: Option<&mut RecordCollectionBudget>,
) -> Result<Option<Vec<u8>>> {
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
    if opened.length_hint > max_record_bytes {
        return Err(PanelError::corrupt_state(format!(
            "snapshot record exceeds the {} byte limit: {}",
            max_record_bytes,
            path.display()
        )));
    }

    let bytes =
        match BoundedRecordReader::new(max_record_bytes).read(opened.file, opened.length_hint) {
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
    if let Some(budget) = collection_budget {
        budget.consume(encoded_bytes).map_err(|error| {
            PanelError::resource_exhausted(format!(
                "prepared snapshot records exceed the {} byte aggregate limit",
                error.max_bytes
            ))
        })?;
    }
    Ok(Some(bytes))
}

fn ensure_prepared_write_capacity(
    directory: &StateDirectoryHandle,
    destination: &OsStr,
    new_record_bytes: usize,
    limits: SnapshotStoreLimits,
) -> Result<()> {
    let max_entries = limits.max_prepared_directory_entries();
    let entries = directory
        .read_entry_names(max_entries.saturating_add(1))
        .map_err(|error| storage_error("read prepared directory", directory.path(), error))?;
    if entries.len() > max_entries {
        return Err(PanelError::resource_exhausted(format!(
            "prepared directory exceeds the {max_entries} entry scan limit"
        )));
    }

    let replacing = entries.iter().any(|name| name == destination);
    if !replacing && entries.len() >= max_entries {
        return Err(PanelError::resource_exhausted(format!(
            "prepared directory has reached the {max_entries} entry limit"
        )));
    }

    let mut budget = RecordCollectionBudget::new(limits.max_prepared_record_bytes());
    let mut replaced_bytes = 0;
    for name in entries {
        if Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            != Some("json")
        {
            continue;
        }
        let path = directory.path_for(&name);
        let bytes = read_record_bytes(
            directory,
            &name,
            limits.max_record_bytes(),
            Some(&mut budget),
        )?
        .ok_or_else(|| {
            PanelError::corrupt_state(format!(
                "prepared snapshot disappeared while measuring {}",
                path.display()
            ))
        })?;
        if name == destination {
            replaced_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        }
    }

    let new_record_bytes = u64::try_from(new_record_bytes).unwrap_or(u64::MAX);
    let projected = budget
        .consumed_bytes()
        .saturating_sub(replaced_bytes)
        .checked_add(new_record_bytes)
        .unwrap_or(u64::MAX);
    if projected > limits.max_prepared_record_bytes() {
        return Err(PanelError::resource_exhausted(format!(
            "prepared snapshot records would exceed the {} byte aggregate limit",
            limits.max_prepared_record_bytes()
        )));
    }
    Ok(())
}

fn atomic_write_record<T: Serialize>(
    directory: &StateDirectoryHandle,
    name: &OsStr,
    payload: &T,
    codecs: &SnapshotRecordCodecRegistry,
    max_record_bytes: u64,
) -> Result<()> {
    let bytes = encode_record(payload, codecs, max_record_bytes)?;
    publish_record(directory, name, &bytes)
}

fn encode_record<T: Serialize>(
    payload: &T,
    codecs: &SnapshotRecordCodecRegistry,
    max_record_bytes: u64,
) -> Result<Vec<u8>> {
    let bytes = codecs.encode(payload)?;
    let encoded_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if encoded_bytes > max_record_bytes {
        return Err(PanelError::resource_exhausted(format!(
            "snapshot record exceeds the {max_record_bytes} byte limit"
        )));
    }

    Ok(bytes)
}

fn publish_record(directory: &StateDirectoryHandle, name: &OsStr, bytes: &[u8]) -> Result<()> {
    AtomicFilePublisher::new(directory, SNAPSHOT_TEMPORARY_PREFIX)
        .publish_bytes(name, bytes)
        .map_err(snapshot_publish_error)
}

/// Rejects a limit set whose ceilings contradict each other.
///
/// Every fallible constructor routes through here, so a contradictory limit set
/// is refused at composition rather than surfacing later as a prepare that can
/// never succeed.
fn usable_limits(limits: SnapshotStoreLimits) -> Result<()> {
    limits.validate().map_err(|error| {
        PanelError::invalid_argument(format!("snapshot store limits are unusable: {error}"))
    })
}

fn reclaim_temporaries(directory: &StateDirectoryHandle, max_entries: usize) -> Result<()> {
    AtomicFilePublisher::new(directory, SNAPSHOT_TEMPORARY_PREFIX)
        .reclaim_abandoned(max_entries)
        .map(|_reclaimed| ())
        .map_err(snapshot_publish_error)
}

fn snapshot_publish_error(error: AtomicPublishError) -> PanelError {
    let (stage, path, source) = error.into_parts();
    let operation = match stage {
        AtomicPublishStage::CreateTemporary => "create temporary snapshot",
        AtomicPublishStage::WriteTemporary => "write temporary snapshot",
        AtomicPublishStage::SyncTemporary => "sync temporary snapshot",
        AtomicPublishStage::Activate => "activate snapshot record",
        AtomicPublishStage::SyncDirectory => "sync snapshot directory",
        AtomicPublishStage::Reclaim => "reclaim abandoned snapshot temporaries",
    };
    storage_error(operation, &path, source)
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
    use std::{
        fs,
        num::{NonZeroU64, NonZeroUsize},
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct JsonSnapshotRecordCodecV2;

    struct CountingSnapshotRecordCodecV1 {
        decodes: Arc<AtomicUsize>,
    }

    impl SnapshotRecordCodec for CountingSnapshotRecordCodecV1 {
        fn format_version(&self) -> u32 {
            JSON_SNAPSHOT_RECORD_FORMAT_V1
        }

        fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
            JsonSnapshotRecordCodecV1.encode_payload(payload)
        }

        fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>> {
            self.decodes.fetch_add(1, Ordering::Relaxed);
            JsonSnapshotRecordCodecV1.decode_payload(record)
        }
    }

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

    fn limits_with_record_bytes(max_record_bytes: u64) -> SnapshotStoreLimits {
        SnapshotStoreLimits::new(
            NonZeroU64::new(max_record_bytes).unwrap(),
            NonZeroU64::new(max_record_bytes.saturating_mul(4)).unwrap(),
            NonZeroUsize::new(16).unwrap(),
        )
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
    async fn encoded_record_limit_is_symmetric_for_writes_and_reads() {
        let temporary = TemporaryDirectory::new();
        let codecs = Arc::new(SnapshotRecordCodecRegistry::default());
        let prepared = prepared_record(1);
        let encoded_bytes = u64::try_from(codecs.encode(&prepared).unwrap().len()).unwrap();
        let store = FileSnapshotStore::with_codec_registry_and_limits(
            &temporary.0,
            Arc::clone(&codecs),
            limits_with_record_bytes(encoded_bytes),
        );

        store.save_prepared(prepared.clone()).await.unwrap();
        assert_eq!(store.load_prepared().await.unwrap(), [prepared]);
    }

    #[tokio::test]
    async fn codec_expansion_cannot_persist_an_unreadable_record() {
        let temporary = TemporaryDirectory::new();
        let codecs = Arc::new(SnapshotRecordCodecRegistry::default());
        let prepared = prepared_record(1);
        let encoded_bytes = u64::try_from(codecs.encode(&prepared).unwrap().len()).unwrap();
        let store = FileSnapshotStore::with_codec_registry_and_limits(
            &temporary.0,
            codecs,
            limits_with_record_bytes(encoded_bytes - 1),
        );

        let error = store.save_prepared(prepared).await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        assert!(store.load_prepared().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn prepared_write_cannot_exceed_the_aggregate_read_budget() {
        let temporary = TemporaryDirectory::new();
        let codecs = Arc::new(SnapshotRecordCodecRegistry::default());
        let first = prepared_record(1);
        let second = prepared_record(2);
        let record_ceiling = [
            codecs.encode(&first).unwrap().len(),
            codecs.encode(&second).unwrap().len(),
        ]
        .into_iter()
        .max()
        .unwrap() as u64;
        let limits = SnapshotStoreLimits::try_new(
            NonZeroU64::new(record_ceiling).unwrap(),
            NonZeroU64::new(record_ceiling).unwrap(),
            NonZeroUsize::new(8).unwrap(),
        )
        .unwrap();
        let store = FileSnapshotStore::with_codec_registry_and_limits(&temporary.0, codecs, limits);

        store.save_prepared(first.clone()).await.unwrap();
        let error = store.save_prepared(second).await.unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        assert_eq!(store.load_prepared().await.unwrap(), [first]);
    }

    #[tokio::test]
    async fn prepared_write_cannot_exceed_the_directory_entry_budget() {
        let temporary = TemporaryDirectory::new();
        let limits = SnapshotStoreLimits::try_new(
            NonZeroU64::new(4096).unwrap(),
            NonZeroU64::new(16_384).unwrap(),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
        let store = FileSnapshotStore::with_limits(&temporary.0, limits);
        let first = prepared_record(1);

        store.save_prepared(first.clone()).await.unwrap();
        let error = store.save_prepared(prepared_record(2)).await.unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        assert_eq!(store.load_prepared().await.unwrap(), [first]);
    }

    #[tokio::test]
    async fn replacing_a_prepared_record_is_allowed_at_capacity() {
        let temporary = TemporaryDirectory::new();
        let codecs = Arc::new(SnapshotRecordCodecRegistry::default());
        let prepared = prepared_record(1);
        let encoded_bytes = u64::try_from(codecs.encode(&prepared).unwrap().len()).unwrap();
        let limits = SnapshotStoreLimits::try_new(
            NonZeroU64::new(encoded_bytes).unwrap(),
            NonZeroU64::new(encoded_bytes).unwrap(),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
        let store = FileSnapshotStore::with_codec_registry_and_limits(&temporary.0, codecs, limits);

        store.save_prepared(prepared.clone()).await.unwrap();
        store.save_prepared(prepared.clone()).await.unwrap();

        assert_eq!(store.load_prepared().await.unwrap(), [prepared]);
    }

    #[tokio::test]
    async fn prepared_aggregate_budget_is_charged_before_codec_decoding() {
        let temporary = TemporaryDirectory::new();
        let prepared = prepared_record(1);
        FileSnapshotStore::new(&temporary.0)
            .save_prepared(prepared.clone())
            .await
            .unwrap();
        let path = FileSnapshotStore::prepared_path(&temporary.0, &prepared.receipt.prepare_token);
        let encoded_bytes = fs::metadata(path).unwrap().len();
        let decodes = Arc::new(AtomicUsize::new(0));
        let codecs = SnapshotRecordCodecRegistry::new(Arc::new(CountingSnapshotRecordCodecV1 {
            decodes: Arc::clone(&decodes),
        }))
        .unwrap();
        let limits = SnapshotStoreLimits::new(
            NonZeroU64::new(encoded_bytes).unwrap(),
            NonZeroU64::new(encoded_bytes - 1).unwrap(),
            NonZeroUsize::new(16).unwrap(),
        );
        let store = FileSnapshotStore::with_codec_registry_and_limits(
            &temporary.0,
            Arc::new(codecs),
            limits,
        );

        let error = store.load_prepared().await.unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        assert_eq!(decodes.load(Ordering::Relaxed), 0);
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

    #[tokio::test]
    async fn opening_exclusively_reclaims_crash_orphaned_temporaries() {
        let temporary = TemporaryDirectory::new();
        let prepared = FileSnapshotStore::prepared_directory(&temporary.0);
        std::fs::create_dir_all(&prepared).unwrap();
        let orphans = [
            temporary
                .0
                .join(".snapshot-00000000-0000-4000-8000-000000000001.tmp"),
            prepared.join(".snapshot-00000000-0000-4000-8000-000000000002.tmp"),
        ];
        let retained = [
            temporary.0.join(ACTIVE_FILE_NAME),
            prepared.join("keep.json"),
            temporary.0.join(".unrelated-4a2f.tmp"),
            temporary.0.join(".snapshot-not-a-uuid.tmp"),
        ];
        for path in orphans.iter().chain(retained.iter()) {
            std::fs::write(path, b"{}").unwrap();
        }

        let store = FileSnapshotStore::open_exclusive(&temporary.0)
            .await
            .unwrap();

        assert!(store.has_exclusive_lease());
        for orphan in &orphans {
            assert!(!orphan.exists(), "{} was not reclaimed", orphan.display());
        }
        for kept in &retained {
            assert!(kept.exists(), "{} was reclaimed", kept.display());
        }
    }

    #[tokio::test]
    async fn opening_exclusively_rejects_a_contradictory_limit_set() {
        let temporary = TemporaryDirectory::new();
        let limits = SnapshotStoreLimits::new(
            std::num::NonZeroU64::new(4096).unwrap(),
            std::num::NonZeroU64::new(4095).unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
        );

        let error = FileSnapshotStore::open_exclusive_with_limits(&temporary.0, limits)
            .await
            .unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::INVALID_ARGUMENT);
        // Rejection happens before the state directory is claimed, so a second
        // attempt with usable limits is not blocked by a leaked lease.
        assert!(FileSnapshotStore::open_exclusive(&temporary.0)
            .await
            .unwrap()
            .has_exclusive_lease());
    }

    #[test]
    fn composing_a_store_fallibly_rejects_a_contradictory_limit_set() {
        let temporary = TemporaryDirectory::new();
        let limits = SnapshotStoreLimits::new(
            std::num::NonZeroU64::new(4096).unwrap(),
            std::num::NonZeroU64::new(4095).unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
        );

        let error = FileSnapshotStore::try_with_limits(&temporary.0, limits).unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::INVALID_ARGUMENT);

        let error = FileSnapshotStore::try_with_codec_registry_and_limits(
            &temporary.0,
            Arc::new(SnapshotRecordCodecRegistry::default()),
            limits,
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::INVALID_ARGUMENT);

        // The infallible constructors keep their existing behaviour, which is
        // the whole reason the fallible ones were added beside them.
        assert_eq!(
            FileSnapshotStore::with_limits(&temporary.0, limits).limits(),
            limits
        );
    }

    #[test]
    fn composing_a_store_fallibly_accepts_a_usable_limit_set() {
        let temporary = TemporaryDirectory::new();
        let limits = SnapshotStoreLimits::try_new(
            std::num::NonZeroU64::new(4096).unwrap(),
            std::num::NonZeroU64::new(4096).unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
        )
        .unwrap();

        let store = FileSnapshotStore::try_with_limits(&temporary.0, limits).unwrap();

        assert_eq!(store.limits(), limits);
        assert!(!store.has_exclusive_lease());
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
        for index in 0..=DEFAULT_MAX_PREPARED_DIRECTORY_ENTRIES {
            fs::write(directory.join(format!("ignored-{index}.tmp")), []).unwrap();
        }

        let store = FileSnapshotStore::new(&temporary.0);
        let error = store
            .load_prepared_bounded(DEFAULT_MAX_PREPARED_DIRECTORY_ENTRIES)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
    }
}
