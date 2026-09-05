#![forbid(unsafe_code)]

use async_trait::async_trait;
use panel_domain::RevisionId;
use panel_engine::{
    ActivationCommitOutcome, ActiveSnapshotRecord, DataPlaneAdapter, EngineCapabilities,
    GatewayEngine, PrepareRequest, PrepareToken, PreparedSnapshotRecord, SnapshotStore,
};
use panel_errors::{Result, ValidationReport};
use panel_gateway_runtime::DurableGatewayEngine;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use snapshot_store_fs::FileSnapshotStore;
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Notify;
use uuid::Uuid;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pingora-panel-filesystem-cancellation-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestAdapter;

#[async_trait]
impl DataPlaneAdapter for TestAdapter {
    type Prepared = RuntimeSnapshot;

    async fn capabilities(&self) -> Result<EngineCapabilities> {
        Ok(EngineCapabilities {
            protocol_version: "test.v1".into(),
            build_version: "test".into(),
            schema_version: IR_SCHEMA_VERSION.into(),
            adapter_version: "filesystem-test".into(),
            capabilities: BTreeSet::new(),
        })
    }

    async fn validate(&self, _snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
        Ok(ValidationReport::valid())
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<Self::Prepared> {
        Ok(snapshot)
    }

    fn activate(&self, _prepared: Arc<Self::Prepared>) {}
}

struct PausingFileStore {
    inner: FileSnapshotStore,
    pause_save: AtomicBool,
    pause_delete: AtomicBool,
    save_started: Notify,
    delete_started: Notify,
    continue_save: Notify,
    continue_delete: Notify,
}

impl PausingFileStore {
    fn new(root: PathBuf) -> Self {
        Self {
            inner: FileSnapshotStore::new(root),
            pause_save: AtomicBool::new(false),
            pause_delete: AtomicBool::new(false),
            save_started: Notify::new(),
            delete_started: Notify::new(),
            continue_save: Notify::new(),
            continue_delete: Notify::new(),
        }
    }
}

#[async_trait]
impl SnapshotStore for PausingFileStore {
    async fn load_active(&self) -> Result<Option<ActiveSnapshotRecord>> {
        self.inner.load_active().await
    }

    async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
        self.inner.load_prepared().await
    }

    async fn load_prepared_bounded(&self, limit: usize) -> Result<Vec<PreparedSnapshotRecord>> {
        self.inner.load_prepared_bounded(limit).await
    }

    async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
        if self.pause_save.load(Ordering::Acquire) {
            self.save_started.notify_one();
            self.continue_save.notified().await;
        }
        self.inner.save_prepared(record).await
    }

    async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
        if self.pause_delete.load(Ordering::Acquire) {
            self.delete_started.notify_one();
            self.continue_delete.notified().await;
        }
        self.inner.delete_prepared(token).await
    }

    async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()> {
        self.inner.commit_activation(record).await
    }

    async fn commit_activation_with_outcome(
        &self,
        record: ActiveSnapshotRecord,
    ) -> Result<ActivationCommitOutcome> {
        self.inner.commit_activation_with_outcome(record).await
    }
}

fn snapshot(revision: u64) -> RuntimeSnapshot {
    RuntimeSnapshot::empty(RevisionId::new(revision))
}

#[tokio::test]
async fn cancelled_prepare_finishes_the_filesystem_transaction() {
    let temporary = TemporaryDirectory::new();
    let store = Arc::new(PausingFileStore::new(temporary.0.clone()));
    store.pause_save.store(true, Ordering::Release);
    let engine = Arc::new(
        DurableGatewayEngine::restore(Arc::new(TestAdapter), Arc::clone(&store))
            .await
            .unwrap(),
    );

    let request = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .prepare(PrepareRequest {
                    snapshot: snapshot(1),
                })
                .await
        })
    };
    store.save_started.notified().await;
    request.abort();
    store.continue_save.notify_one();

    for _ in 0..100 {
        if store.inner.load_prepared().await.unwrap().len() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(store.inner.load_prepared().await.unwrap().len(), 1);
    assert_eq!(engine.status().await.unwrap().prepared_count, 1);
}

#[tokio::test]
async fn cancelled_abort_finishes_the_filesystem_transaction() {
    let temporary = TemporaryDirectory::new();
    let store = Arc::new(PausingFileStore::new(temporary.0.clone()));
    let engine = Arc::new(
        DurableGatewayEngine::restore(Arc::new(TestAdapter), Arc::clone(&store))
            .await
            .unwrap(),
    );
    let prepared = engine
        .prepare(PrepareRequest {
            snapshot: snapshot(1),
        })
        .await
        .unwrap();
    store.pause_delete.store(true, Ordering::Release);

    let request = {
        let engine = Arc::clone(&engine);
        let token = prepared.prepare_token;
        tokio::spawn(async move { engine.abort(token).await })
    };
    store.delete_started.notified().await;
    request.abort();
    store.continue_delete.notify_one();

    for _ in 0..100 {
        if store.inner.load_prepared().await.unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(store.inner.load_prepared().await.unwrap().is_empty());
    assert_eq!(engine.status().await.unwrap().prepared_count, 0);
}
