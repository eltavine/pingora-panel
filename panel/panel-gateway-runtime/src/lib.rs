//! Durable gateway use-case orchestration.
//!
//! This crate owns no transport, filesystem, or Pingora types. Those details enter
//! through `SnapshotStore` and `DataPlaneAdapter` ports at the composition root.

use async_trait::async_trait;
use panel_engine::{
    AbortReceipt, ActivateRequest, ActivationReceipt, ActiveSnapshotRecord, DataPlaneAdapter,
    EngineCapabilities, GatewayEngine, GatewayStatus, PrepareReceipt, PrepareRequest, PrepareToken,
    PreparedSnapshotRecord, SnapshotEnvelope, SnapshotStore,
};
use panel_errors::{ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

struct PreparedEntry<P> {
    record: PreparedSnapshotRecord,
    artifact: Arc<P>,
}

struct RuntimeState<P> {
    active: Option<ActiveSnapshotRecord>,
    prepared: HashMap<PrepareToken, PreparedEntry<P>>,
    degraded: Option<PanelError>,
}

impl<P> Default for RuntimeState<P> {
    fn default() -> Self {
        Self {
            active: None,
            prepared: HashMap::new(),
            degraded: None,
        }
    }
}

pub struct DurableGatewayEngine<A, S>
where
    A: DataPlaneAdapter,
    S: SnapshotStore,
{
    adapter: Arc<A>,
    store: Arc<S>,
    state: Arc<Mutex<RuntimeState<A::Prepared>>>,
}

impl<A, S> DurableGatewayEngine<A, S>
where
    A: DataPlaneAdapter,
    S: SnapshotStore,
{
    /// Restore the active snapshot and any outstanding prepare tokens before the
    /// gateway accepts mutations. Corrupt or incompatible state fails closed.
    pub async fn restore(adapter: Arc<A>, store: Arc<S>) -> Result<Self> {
        let mut state = RuntimeState::default();
        match store.load_active().await {
            Ok(Some(record)) => match adapter.prepare(record.envelope.snapshot.clone()).await {
                Ok(artifact) => {
                    adapter.activate(Arc::new(artifact));
                    state.active = Some(record);
                }
                Err(error) => state.degraded = Some(error),
            },
            Ok(None) => {}
            Err(error) => state.degraded = Some(error),
        }

        if state.degraded.is_none() {
            match store.load_prepared().await {
                Ok(records) => {
                    for record in records {
                        if state.active.as_ref().is_some_and(|active| {
                            record.envelope.snapshot.revision_id
                                <= active.envelope.snapshot.revision_id
                        }) {
                            if let Err(error) =
                                store.delete_prepared(&record.receipt.prepare_token).await
                            {
                                state.degraded = Some(error);
                                break;
                            }
                            continue;
                        }
                        if state.prepared.contains_key(&record.receipt.prepare_token) {
                            state.degraded = Some(PanelError::corrupt_state(
                                "duplicate prepare token in snapshot store",
                            ));
                            break;
                        }
                        match adapter.prepare(record.envelope.snapshot.clone()).await {
                            Ok(artifact) => {
                                state.prepared.insert(
                                    record.receipt.prepare_token.clone(),
                                    PreparedEntry {
                                        record,
                                        artifact: Arc::new(artifact),
                                    },
                                );
                            }
                            Err(error) => {
                                state.degraded = Some(error);
                                break;
                            }
                        }
                    }
                }
                Err(error) => state.degraded = Some(error),
            }
        }

        Ok(Self {
            adapter,
            store,
            state: Arc::new(Mutex::new(state)),
        })
    }

    fn ensure_prepare_is_new(
        state: &RuntimeState<A::Prepared>,
        snapshot: &RuntimeSnapshot,
    ) -> Result<()> {
        if let Some(active) = &state.active {
            if snapshot.revision_id <= active.envelope.snapshot.revision_id {
                return Err(PanelError::conflict(
                    "revision is not newer than the active revision",
                ));
            }
        }
        if state.prepared.values().any(|item| {
            item.record.envelope.snapshot.revision_id == snapshot.revision_id
                || item.record.envelope.snapshot.content_hash == snapshot.content_hash
        }) {
            return Err(PanelError::conflict(
                "revision or content is already prepared",
            ));
        }
        Ok(())
    }

    fn ensure_mutations_allowed(state: &RuntimeState<A::Prepared>) -> Result<()> {
        if let Some(error) = &state.degraded {
            return Err(PanelError::precondition_failed(format!(
                "gateway startup state requires operator recovery: {}",
                error.code
            )));
        }
        Ok(())
    }

    async fn activate_transaction(
        adapter: Arc<A>,
        store: Arc<S>,
        state: Arc<Mutex<RuntimeState<A::Prepared>>>,
        request: ActivateRequest,
    ) -> Result<ActivationReceipt> {
        let mut state = state.lock().await;
        Self::ensure_mutations_allowed(&state)?;

        if let Some(active) = &state.active {
            if active.receipt.prepare_token == request.prepare_token {
                if active.receipt.previous_active_hash == request.expected_active_hash {
                    let receipt = active.receipt.clone();
                    store.delete_prepared(&request.prepare_token).await?;
                    return Ok(receipt);
                }
                return Err(PanelError::conflict(
                    "idempotent activation retry used a different expected active hash",
                ));
            }
        }

        let current_hash = state
            .active
            .as_ref()
            .map(|item| item.envelope.snapshot.content_hash.clone());
        if current_hash != request.expected_active_hash {
            return Err(PanelError::conflict(
                "expected active hash does not match current active hash",
            ));
        }

        let prepared = state
            .prepared
            .get(&request.prepare_token)
            .ok_or_else(|| PanelError::new(ErrorCode::NOT_FOUND, "prepare token was not found"))?;
        if state.active.as_ref().is_some_and(|active| {
            prepared.record.envelope.snapshot.revision_id <= active.envelope.snapshot.revision_id
        }) {
            return Err(PanelError::conflict("prepared revision is stale"));
        }

        let receipt = ActivationReceipt {
            revision_id: prepared.record.envelope.snapshot.revision_id,
            content_hash: prepared.record.envelope.snapshot.content_hash.clone(),
            adapter_version: prepared.record.receipt.adapter_version.clone(),
            schema_version: prepared.record.envelope.snapshot.schema_version.clone(),
            prepare_token: request.prepare_token.clone(),
            previous_active_hash: current_hash,
        };
        let active = ActiveSnapshotRecord {
            envelope: prepared.record.envelope.clone(),
            receipt: receipt.clone(),
        };
        let artifact = Arc::clone(&prepared.artifact);

        // The durable record is committed before the infallible data-plane pointer
        // swap. This task is detached from the request future so cancellation cannot
        // strand a committed receipt behind an old in-memory pointer.
        store.commit_activation(active.clone()).await?;
        adapter.activate(artifact);
        state.active = Some(active);
        state.prepared.remove(&request.prepare_token);

        // If cleanup fails the activation is intentionally reported as unknown.
        // Retrying the same token returns the persisted receipt idempotently.
        store.delete_prepared(&request.prepare_token).await?;
        Ok(receipt)
    }
}

#[async_trait]
impl<A, S> GatewayEngine for DurableGatewayEngine<A, S>
where
    A: DataPlaneAdapter + 'static,
    S: SnapshotStore + 'static,
{
    async fn capabilities(&self) -> Result<EngineCapabilities> {
        self.adapter.capabilities().await
    }

    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        self.adapter.validate(&snapshot).await
    }

    async fn prepare(&self, request: PrepareRequest) -> Result<PrepareReceipt> {
        {
            let state = self.state.lock().await;
            Self::ensure_mutations_allowed(&state)?;
            Self::ensure_prepare_is_new(&state, &request.snapshot)?;
        }
        let artifact = Arc::new(self.adapter.prepare(request.snapshot.clone()).await?);
        let capabilities = self.adapter.capabilities().await?;
        let mut state = self.state.lock().await;
        Self::ensure_mutations_allowed(&state)?;
        Self::ensure_prepare_is_new(&state, &request.snapshot)?;

        let token = PrepareToken::new(Uuid::new_v4().to_string());
        let receipt = PrepareReceipt {
            revision_id: request.snapshot.revision_id,
            content_hash: request.snapshot.content_hash.clone(),
            adapter_version: capabilities.adapter_version,
            schema_version: request.snapshot.schema_version.clone(),
            prepare_token: token.clone(),
            previous_active_hash: state
                .active
                .as_ref()
                .map(|item| item.envelope.snapshot.content_hash.clone()),
        };
        let record = PreparedSnapshotRecord {
            envelope: SnapshotEnvelope {
                snapshot: request.snapshot,
            },
            receipt: receipt.clone(),
        };
        self.store.save_prepared(record.clone()).await?;
        state
            .prepared
            .insert(token, PreparedEntry { record, artifact });
        Ok(receipt)
    }

    async fn activate(&self, request: ActivateRequest) -> Result<ActivationReceipt> {
        tokio::spawn(Self::activate_transaction(
            Arc::clone(&self.adapter),
            Arc::clone(&self.store),
            Arc::clone(&self.state),
            request,
        ))
        .await
        .map_err(|error| PanelError::internal("activation task failed").with_source(error))?
    }

    async fn abort(&self, token: PrepareToken) -> Result<AbortReceipt> {
        let mut state = self.state.lock().await;
        Self::ensure_mutations_allowed(&state)?;
        let record = state
            .prepared
            .get(&token)
            .ok_or_else(|| PanelError::new(ErrorCode::NOT_FOUND, "prepare token was not found"))?
            .record
            .clone();
        self.store.delete_prepared(&token).await?;
        state.prepared.remove(&token);
        Ok(AbortReceipt {
            revision_id: record.envelope.snapshot.revision_id,
            content_hash: record.envelope.snapshot.content_hash,
            adapter_version: record.receipt.adapter_version,
            schema_version: record.envelope.snapshot.schema_version,
            prepare_token: token,
            previous_active_hash: state
                .active
                .as_ref()
                .map(|item| item.envelope.snapshot.content_hash.clone()),
        })
    }

    async fn status(&self) -> Result<GatewayStatus> {
        let capabilities = self.adapter.capabilities().await?;
        let state = self.state.lock().await;
        Ok(GatewayStatus {
            ready: state.degraded.is_none(),
            message: state
                .degraded
                .as_ref()
                .map(|error| format!("startup recovery required: {}", error.code)),
            active_revision_id: state
                .active
                .as_ref()
                .map(|item| item.envelope.snapshot.revision_id),
            active_hash: state
                .active
                .as_ref()
                .map(|item| item.envelope.snapshot.content_hash.clone()),
            prepared_count: state.prepared.len(),
            adapter_version: capabilities.adapter_version,
            schema_version: capabilities.schema_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::RevisionId;
    use panel_engine::{EngineCapability, SnapshotStore};
    use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
    use std::{collections::BTreeSet, sync::RwLock};
    use tokio::sync::Notify;

    struct TestAdapter {
        active: RwLock<Option<Arc<RuntimeSnapshot>>>,
    }

    impl TestAdapter {
        fn new() -> Self {
            Self {
                active: RwLock::new(None),
            }
        }

        fn active_hash(&self) -> Option<panel_domain::ContentHash> {
            self.active
                .read()
                .unwrap()
                .as_ref()
                .map(|snapshot| snapshot.content_hash.clone())
        }
    }

    #[async_trait]
    impl DataPlaneAdapter for TestAdapter {
        type Prepared = RuntimeSnapshot;

        async fn capabilities(&self) -> Result<EngineCapabilities> {
            Ok(EngineCapabilities {
                protocol_version: "test.v1".into(),
                build_version: "test".into(),
                schema_version: IR_SCHEMA_VERSION.into(),
                adapter_version: "test-adapter".into(),
                capabilities: BTreeSet::<EngineCapability>::new(),
            })
        }

        async fn validate(&self, snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
            if snapshot.has_valid_content_hash() {
                Ok(ValidationReport::valid())
            } else {
                Err(PanelError::validation_failed("invalid hash"))
            }
        }

        async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<Self::Prepared> {
            self.validate(&snapshot).await?;
            Ok(snapshot)
        }

        fn activate(&self, prepared: Arc<Self::Prepared>) {
            *self.active.write().unwrap() = Some(prepared);
        }
    }

    #[derive(Default)]
    struct MemoryStoreState {
        active: Option<ActiveSnapshotRecord>,
        prepared: HashMap<PrepareToken, PreparedSnapshotRecord>,
        fail_commit: bool,
        pause_commit: bool,
        load_error: Option<PanelError>,
    }

    #[derive(Default)]
    struct MemoryStore {
        state: Mutex<MemoryStoreState>,
        commit_started: Notify,
        continue_commit: Notify,
    }

    #[async_trait]
    impl SnapshotStore for MemoryStore {
        async fn load_active(&self) -> Result<Option<ActiveSnapshotRecord>> {
            let state = self.state.lock().await;
            if let Some(error) = &state.load_error {
                return Err(error.clone());
            }
            Ok(state.active.clone())
        }

        async fn load_prepared(&self) -> Result<Vec<PreparedSnapshotRecord>> {
            Ok(self.state.lock().await.prepared.values().cloned().collect())
        }

        async fn save_prepared(&self, record: PreparedSnapshotRecord) -> Result<()> {
            self.state
                .lock()
                .await
                .prepared
                .insert(record.receipt.prepare_token.clone(), record);
            Ok(())
        }

        async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
            self.state.lock().await.prepared.remove(token);
            Ok(())
        }

        async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()> {
            let (fail_commit, pause_commit) = {
                let state = self.state.lock().await;
                (state.fail_commit, state.pause_commit)
            };
            if fail_commit {
                return Err(PanelError::storage_unavailable("injected failure"));
            }
            if pause_commit {
                self.commit_started.notify_one();
                self.continue_commit.notified().await;
            }
            let mut state = self.state.lock().await;
            state.active = Some(record);
            Ok(())
        }
    }

    fn snapshot(revision: u64) -> RuntimeSnapshot {
        RuntimeSnapshot::empty(RevisionId::new(revision))
    }

    #[tokio::test]
    async fn storage_failure_never_switches_the_data_plane() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let engine = DurableGatewayEngine::restore(Arc::clone(&adapter), Arc::clone(&store))
            .await
            .unwrap();
        let prepared = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        store.state.lock().await.fail_commit = true;

        let error = engine
            .activate(ActivateRequest {
                prepare_token: prepared.prepare_token,
                expected_active_hash: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::STORAGE_UNAVAILABLE);
        assert!(adapter.active_hash().is_none());
        assert!(engine.status().await.unwrap().active_hash.is_none());
    }

    #[tokio::test]
    async fn restart_restores_the_last_known_good_snapshot() {
        let store = Arc::new(MemoryStore::default());
        let first_adapter = Arc::new(TestAdapter::new());
        let first = DurableGatewayEngine::restore(first_adapter, Arc::clone(&store))
            .await
            .unwrap();
        let prepared = first
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let receipt = first
            .activate(ActivateRequest {
                prepare_token: prepared.prepare_token,
                expected_active_hash: None,
            })
            .await
            .unwrap();
        drop(first);

        let restored_adapter = Arc::new(TestAdapter::new());
        let restored =
            DurableGatewayEngine::restore(Arc::clone(&restored_adapter), Arc::clone(&store))
                .await
                .unwrap();
        assert_eq!(
            restored_adapter.active_hash(),
            Some(receipt.content_hash.clone())
        );
        assert_eq!(
            restored.status().await.unwrap().active_hash,
            Some(receipt.content_hash)
        );
    }

    #[tokio::test]
    async fn activation_retry_returns_the_persisted_receipt() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let engine = DurableGatewayEngine::restore(adapter, store).await.unwrap();
        let prepared = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let request = ActivateRequest {
            prepare_token: prepared.prepare_token,
            expected_active_hash: None,
        };
        let first = engine.activate(request.clone()).await.unwrap();
        let second = engine.activate(request).await.unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn concurrent_compare_and_swap_has_one_winner() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let engine = Arc::new(DurableGatewayEngine::restore(adapter, store).await.unwrap());
        let first = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let second = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(2),
            })
            .await
            .unwrap();
        let left = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .activate(ActivateRequest {
                        prepare_token: first.prepare_token,
                        expected_active_hash: None,
                    })
                    .await
            })
        };
        let right = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .activate(ActivateRequest {
                        prepare_token: second.prepare_token,
                        expected_active_hash: None,
                    })
                    .await
            })
        };
        let outcomes = [left.await.unwrap(), right.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_err()).count(),
            1
        );
    }

    #[tokio::test]
    async fn corrupt_startup_state_serves_status_but_blocks_mutations() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        store.state.lock().await.load_error = Some(PanelError::corrupt_state("bad manifest"));

        let engine = DurableGatewayEngine::restore(adapter, store).await.unwrap();
        let status = engine.status().await.unwrap();
        assert!(!status.ready);
        assert!(status.message.unwrap().contains("CORRUPT_STATE"));
        let error = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn request_cancellation_cannot_strand_a_committed_activation() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        store.state.lock().await.pause_commit = true;
        let engine = Arc::new(
            DurableGatewayEngine::restore(Arc::clone(&adapter), Arc::clone(&store))
                .await
                .unwrap(),
        );
        let prepared = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let activation = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .activate(ActivateRequest {
                        prepare_token: prepared.prepare_token,
                        expected_active_hash: None,
                    })
                    .await
            })
        };
        store.commit_started.notified().await;
        activation.abort();
        store.continue_commit.notify_one();

        for _ in 0..100 {
            if engine.status().await.unwrap().active_hash.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(engine.status().await.unwrap().active_hash.is_some());
        assert!(adapter.active_hash().is_some());
        assert!(store.state.lock().await.active.is_some());
    }
}
