#![forbid(unsafe_code)]

//! Durable gateway use-case orchestration.
//!
//! This crate owns no transport, filesystem, or Pingora types. Those details enter
//! through `SnapshotStore` and `DataPlaneAdapter` ports at the composition root.

mod events;
mod prepared_policy;

pub use events::{
    BufferedGatewayEventReceiver, BufferedGatewayEventSink, FanoutGatewayEventSink, GatewayEvent,
    GatewayEventDeliveryDiagnostics, GatewayEventDeliveryDiagnosticsProvider,
    GatewayEventDeliveryMonitor, GatewayEventDeliverySnapshot, GatewayEventPanicObserver,
    GatewayRecoveryDiagnostics, GatewayRecoveryDiagnosticsProvider, GatewayRecoveryMonitor,
    GatewayEventSink, GatewayOperation, GatewayRequestMetadata, GatewayRequestOperation,
    GatewayRequestOutcome, NoopGatewayEventSink, PanicIsolatedGatewayEventSink,
};
pub use prepared_policy::{
    PreparedSnapshotAdmissionPolicy, PreparedSnapshotBudget, PreparedSnapshotUsage,
    DEFAULT_MAX_OUTSTANDING_PREPARES, DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES,
    DEFAULT_MAX_TOTAL_PREPARED_BYTES,
};

use async_trait::async_trait;
use panel_engine::{
    AbortReceipt, ActivateRequest, ActivationCommitOutcome, ActivationReceipt,
    ActiveSnapshotRecord, DataPlaneAdapter, EngineCapabilities, GatewayEngine, GatewayStatus,
    PrepareReceipt, PrepareRequest, PrepareToken, PreparedSnapshotRecord, SnapshotEnvelope,
    SnapshotStore,
};
use panel_errors::{ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

struct PreparedEntry<P> {
    record: PreparedSnapshotRecord,
    artifact: Arc<P>,
    accounted_bytes: usize,
}

struct RuntimeState<P> {
    active: Option<ActiveSnapshotRecord>,
    prepared: HashMap<PrepareToken, PreparedEntry<P>>,
    prepared_bytes: usize,
    degraded: Option<PanelError>,
}

impl<P> Default for RuntimeState<P> {
    fn default() -> Self {
        Self {
            active: None,
            prepared: HashMap::new(),
            prepared_bytes: 0,
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
    prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
    events: Arc<dyn GatewayEventSink>,
}

pub struct DurableGatewayEngineOptions {
    prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
    events: Arc<dyn GatewayEventSink>,
}

impl DurableGatewayEngineOptions {
    pub fn new(
        prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
        events: Arc<dyn GatewayEventSink>,
    ) -> Self {
        Self {
            prepared_policy,
            events: Arc::new(PanicIsolatedGatewayEventSink::new(events)),
        }
    }

    pub fn with_prepared_policy(
        mut self,
        prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
    ) -> Self {
        self.prepared_policy = prepared_policy;
        self
    }

    pub fn with_event_sink(mut self, events: Arc<dyn GatewayEventSink>) -> Self {
        self.events = Arc::new(PanicIsolatedGatewayEventSink::new(events));
        self
    }
}

impl Default for DurableGatewayEngineOptions {
    fn default() -> Self {
        Self {
            prepared_policy: Arc::new(PreparedSnapshotBudget::default()),
            events: Arc::new(NoopGatewayEventSink),
        }
    }
}

impl<A, S> DurableGatewayEngine<A, S>
where
    A: DataPlaneAdapter,
    S: SnapshotStore,
{
    /// Restore the active snapshot and any outstanding prepare tokens before the
    /// gateway accepts mutations. Corrupt or incompatible state fails closed.
    pub async fn restore(adapter: Arc<A>, store: Arc<S>) -> Result<Self> {
        Self::restore_with_options(adapter, store, DurableGatewayEngineOptions::default()).await
    }

    pub async fn restore_with_options(
        adapter: Arc<A>,
        store: Arc<S>,
        options: DurableGatewayEngineOptions,
    ) -> Result<Self> {
        let mut state = RuntimeState::default();
        match store.load_active().await {
            Ok(Some(record)) => match adapter.prepare(record.envelope.snapshot.clone()).await {
                Ok(artifact) => {
                    adapter.activate(Arc::new(artifact));
                    state.active = Some(record);
                }
                Err(error) => Self::mark_degraded(
                    &mut state,
                    options.events.as_ref(),
                    GatewayOperation::RestoreActive,
                    &error,
                ),
            },
            Ok(None) => {}
            Err(error) => {
                Self::mark_degraded(
                    &mut state,
                    options.events.as_ref(),
                    GatewayOperation::RestoreActive,
                    &error,
                );
            }
        }

        if state.degraded.is_none() {
            match store
                .load_prepared_bounded(options.prepared_policy.restoration_limit())
                .await
            {
                Ok(records) => {
                    for record in records {
                        if state.active.as_ref().is_some_and(|active| {
                            record.envelope.snapshot.revision_id
                                <= active.envelope.snapshot.revision_id
                        }) {
                            if let Err(error) =
                                store.delete_prepared(&record.receipt.prepare_token).await
                            {
                                Self::mark_degraded(
                                    &mut state,
                                    options.events.as_ref(),
                                    GatewayOperation::DeletePrepared,
                                    &error,
                                );
                                break;
                            }
                            continue;
                        }
                        if state.prepared.contains_key(&record.receipt.prepare_token) {
                            let error = PanelError::corrupt_state(
                                "duplicate prepare token in snapshot store",
                            );
                            Self::mark_degraded(
                                &mut state,
                                options.events.as_ref(),
                                GatewayOperation::RestorePrepared,
                                &error,
                            );
                            break;
                        }
                        if let Err(error) = options.prepared_policy.admit(
                            PreparedSnapshotUsage {
                                outstanding: state.prepared.len(),
                                total_bytes: state.prepared_bytes,
                            },
                            &record.envelope.snapshot,
                        ) {
                            Self::mark_degraded(
                                &mut state,
                                options.events.as_ref(),
                                GatewayOperation::RestorePrepared,
                                &error,
                            );
                            break;
                        }
                        let accounted_bytes = record.envelope.snapshot.canonical_bytes().len();
                        let Some(prepared_bytes) =
                            state.prepared_bytes.checked_add(accounted_bytes)
                        else {
                            let error = PanelError::resource_exhausted(
                                "prepared snapshot aggregate size overflow",
                            );
                            Self::mark_degraded(
                                &mut state,
                                options.events.as_ref(),
                                GatewayOperation::RestorePrepared,
                                &error,
                            );
                            break;
                        };
                        match adapter.prepare(record.envelope.snapshot.clone()).await {
                            Ok(artifact) => {
                                state.prepared_bytes = prepared_bytes;
                                state.prepared.insert(
                                    record.receipt.prepare_token.clone(),
                                    PreparedEntry {
                                        record,
                                        artifact: Arc::new(artifact),
                                        accounted_bytes,
                                    },
                                );
                            }
                            Err(error) => {
                                Self::mark_degraded(
                                    &mut state,
                                    options.events.as_ref(),
                                    GatewayOperation::RestorePrepared,
                                    &error,
                                );
                                break;
                            }
                        }
                    }
                }
                Err(error) => Self::mark_degraded(
                    &mut state,
                    options.events.as_ref(),
                    GatewayOperation::RestorePrepared,
                    &error,
                ),
            }
        }

        options.events.emit(&GatewayEvent::RecoveryCompleted {
            ready: state.degraded.is_none(),
            active_revision_id: state
                .active
                .as_ref()
                .map(|record| record.envelope.snapshot.revision_id),
            prepared_count: state.prepared.len(),
        });

        Ok(Self {
            adapter,
            store,
            state: Arc::new(Mutex::new(state)),
            prepared_policy: options.prepared_policy,
            events: options.events,
        })
    }

    fn mark_degraded(
        state: &mut RuntimeState<A::Prepared>,
        events: &dyn GatewayEventSink,
        operation: GatewayOperation,
        error: &PanelError,
    ) {
        state.degraded = Some(error.clone());
        events.emit(&GatewayEvent::Degraded {
            operation,
            error_code: error.code.clone(),
        });
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
                "gateway state requires operator recovery: {}",
                error.code
            )));
        }
        Ok(())
    }

    async fn prepare_transaction(
        adapter: Arc<A>,
        store: Arc<S>,
        state: Arc<Mutex<RuntimeState<A::Prepared>>>,
        prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
        events: Arc<dyn GatewayEventSink>,
        request: PrepareRequest,
    ) -> Result<PrepareReceipt> {
        {
            let state = state.lock().await;
            Self::ensure_mutations_allowed(&state)?;
            Self::ensure_prepare_is_new(&state, &request.snapshot)?;
            prepared_policy.admit(
                PreparedSnapshotUsage {
                    outstanding: state.prepared.len(),
                    total_bytes: state.prepared_bytes,
                },
                &request.snapshot,
            )?;
        }
        let artifact = Arc::new(adapter.prepare(request.snapshot.clone()).await?);
        let capabilities = adapter.capabilities().await?;
        let mut state = state.lock().await;
        Self::ensure_mutations_allowed(&state)?;
        Self::ensure_prepare_is_new(&state, &request.snapshot)?;
        prepared_policy.admit(
            PreparedSnapshotUsage {
                outstanding: state.prepared.len(),
                total_bytes: state.prepared_bytes,
            },
            &request.snapshot,
        )?;
        let accounted_bytes = request.snapshot.canonical_bytes().len();
        let prepared_bytes = state
            .prepared_bytes
            .checked_add(accounted_bytes)
            .ok_or_else(|| {
                PanelError::resource_exhausted("prepared snapshot aggregate size overflow")
            })?;

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
        if let Err(error) = store.save_prepared(record.clone()).await {
            Self::mark_degraded(
                &mut state,
                events.as_ref(),
                GatewayOperation::SavePrepared,
                &error,
            );
            return Err(error);
        }
        state.prepared_bytes = prepared_bytes;
        state.prepared.insert(
            token,
            PreparedEntry {
                record,
                artifact,
                accounted_bytes,
            },
        );
        events.emit(&GatewayEvent::Prepared {
            revision_id: receipt.revision_id,
            prepared_count: state.prepared.len(),
        });
        Ok(receipt)
    }

    async fn abort_transaction(
        store: Arc<S>,
        state: Arc<Mutex<RuntimeState<A::Prepared>>>,
        events: Arc<dyn GatewayEventSink>,
        token: PrepareToken,
    ) -> Result<AbortReceipt> {
        let mut state = state.lock().await;
        Self::ensure_mutations_allowed(&state)?;
        let record = state
            .prepared
            .get(&token)
            .ok_or_else(|| PanelError::new(ErrorCode::NOT_FOUND, "prepare token was not found"))?
            .record
            .clone();
        if let Err(error) = store.delete_prepared(&token).await {
            Self::mark_degraded(
                &mut state,
                events.as_ref(),
                GatewayOperation::DeletePrepared,
                &error,
            );
            return Err(error);
        }
        let removed = state
            .prepared
            .remove(&token)
            .expect("aborted prepare token remains present while state is locked");
        state.prepared_bytes = state.prepared_bytes.saturating_sub(removed.accounted_bytes);
        let receipt = AbortReceipt {
            revision_id: record.envelope.snapshot.revision_id,
            content_hash: record.envelope.snapshot.content_hash,
            adapter_version: record.receipt.adapter_version,
            schema_version: record.envelope.snapshot.schema_version,
            prepare_token: token,
            previous_active_hash: state
                .active
                .as_ref()
                .map(|item| item.envelope.snapshot.content_hash.clone()),
        };
        events.emit(&GatewayEvent::Aborted {
            revision_id: receipt.revision_id,
            prepared_count: state.prepared.len(),
        });
        Ok(receipt)
    }

    async fn activate_transaction(
        adapter: Arc<A>,
        store: Arc<S>,
        state: Arc<Mutex<RuntimeState<A::Prepared>>>,
        events: Arc<dyn GatewayEventSink>,
        request: ActivateRequest,
    ) -> Result<ActivationReceipt> {
        let mut state = state.lock().await;
        Self::ensure_mutations_allowed(&state)?;

        if let Some(active) = &state.active {
            if active.receipt.prepare_token == request.prepare_token {
                if active.receipt.previous_active_hash == request.expected_active_hash {
                    let receipt = active.receipt.clone();
                    if let Err(error) = store.delete_prepared(&request.prepare_token).await {
                        Self::mark_degraded(
                            &mut state,
                            events.as_ref(),
                            GatewayOperation::DeletePrepared,
                            &error,
                        );
                        return Err(error);
                    }
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
        // swap. `activate` runs this transaction in a detached task, so request
        // cancellation cannot strand a committed receipt behind an old pointer.
        let durability_unknown = match store.commit_activation_with_outcome(active.clone()).await {
            Ok(ActivationCommitOutcome::Committed) => None,
            Ok(ActivationCommitOutcome::DurabilityUnknown(error)) => Some(error),
            Ok(_) => {
                let error = PanelError::internal(
                    "snapshot store returned an unsupported activation commit outcome",
                );
                Self::mark_degraded(
                    &mut state,
                    events.as_ref(),
                    GatewayOperation::CommitActivation,
                    &error,
                );
                return Err(error);
            }
            Err(error) => {
                Self::mark_degraded(
                    &mut state,
                    events.as_ref(),
                    GatewayOperation::CommitActivation,
                    &error,
                );
                return Err(error);
            }
        };
        adapter.activate(artifact);
        state.active = Some(active);
        let removed = state
            .prepared
            .remove(&request.prepare_token)
            .expect("activated prepare token remains present while state is locked");
        state.prepared_bytes = state.prepared_bytes.saturating_sub(removed.accounted_bytes);
        events.emit(&GatewayEvent::Activated {
            revision_id: receipt.revision_id,
            prepared_count: state.prepared.len(),
        });

        // The rename already made this activation visible to readers. Keep the
        // data plane and in-memory state aligned with that namespace, then stop
        // mutations and report the outcome as unknown until recovery verifies
        // whichever record survived a crash.
        if let Some(error) = durability_unknown {
            Self::mark_degraded(
                &mut state,
                events.as_ref(),
                GatewayOperation::CommitActivation,
                &error,
            );
            return Err(error);
        }

        // If cleanup fails the activation is intentionally reported as unknown.
        // Retrying the same token returns the persisted receipt idempotently.
        if let Err(error) = store.delete_prepared(&request.prepare_token).await {
            Self::mark_degraded(
                &mut state,
                events.as_ref(),
                GatewayOperation::DeletePrepared,
                &error,
            );
            return Err(error);
        }
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
        tokio::spawn(Self::prepare_transaction(
            Arc::clone(&self.adapter),
            Arc::clone(&self.store),
            Arc::clone(&self.state),
            Arc::clone(&self.prepared_policy),
            Arc::clone(&self.events),
            request,
        ))
        .await
        .map_err(|error| PanelError::internal("prepare task failed").with_source(error))?
    }

    async fn activate(&self, request: ActivateRequest) -> Result<ActivationReceipt> {
        tokio::spawn(Self::activate_transaction(
            Arc::clone(&self.adapter),
            Arc::clone(&self.store),
            Arc::clone(&self.state),
            Arc::clone(&self.events),
            request,
        ))
        .await
        .map_err(|error| PanelError::internal("activation task failed").with_source(error))?
    }

    async fn abort(&self, token: PrepareToken) -> Result<AbortReceipt> {
        tokio::spawn(Self::abort_transaction(
            Arc::clone(&self.store),
            Arc::clone(&self.state),
            Arc::clone(&self.events),
            token,
        ))
        .await
        .map_err(|error| PanelError::internal("abort task failed").with_source(error))?
    }

    async fn status(&self) -> Result<GatewayStatus> {
        let capabilities = self.adapter.capabilities().await?;
        let state = self.state.lock().await;
        Ok(GatewayStatus {
            ready: state.degraded.is_none(),
            message: state
                .degraded
                .as_ref()
                .map(|error| format!("operator recovery required: {}", error.code)),
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
    use std::{
        collections::BTreeSet,
        sync::{atomic::{AtomicBool, Ordering}, Mutex as StdMutex, RwLock},
    };
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
        commit_durability_unknown: bool,
        pause_commit: bool,
        load_error: Option<PanelError>,
    }

    #[derive(Default)]
    struct MemoryStore {
        state: Mutex<MemoryStoreState>,
        commit_started: Notify,
        continue_commit: Notify,
        save_started: Notify,
        continue_save: Notify,
        delete_started: Notify,
        continue_delete: Notify,
        pause_save: AtomicBool,
        pause_delete: AtomicBool,
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
            let pause_save = self.pause_save.load(Ordering::Acquire);
            if pause_save {
                self.save_started.notify_one();
                self.continue_save.notified().await;
            }
            self.state
                .lock()
                .await
                .prepared
                .insert(record.receipt.prepare_token.clone(), record);
            Ok(())
        }

        async fn delete_prepared(&self, token: &PrepareToken) -> Result<()> {
            let pause_delete = self.pause_delete.load(Ordering::Acquire);
            if pause_delete {
                self.delete_started.notify_one();
                self.continue_delete.notified().await;
            }
            self.state.lock().await.prepared.remove(token);
            Ok(())
        }

        async fn commit_activation(&self, record: ActiveSnapshotRecord) -> Result<()> {
            match self.commit_activation_with_outcome(record).await? {
                ActivationCommitOutcome::Committed => Ok(()),
                ActivationCommitOutcome::DurabilityUnknown(error) => Err(error),
                _ => Err(PanelError::internal(
                    "snapshot store returned an unsupported activation commit outcome",
                )),
            }
        }

        async fn commit_activation_with_outcome(
            &self,
            record: ActiveSnapshotRecord,
        ) -> Result<ActivationCommitOutcome> {
            let (fail_commit, pause_commit, commit_durability_unknown) = {
                let state = self.state.lock().await;
                (
                    state.fail_commit,
                    state.pause_commit,
                    state.commit_durability_unknown,
                )
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
            if commit_durability_unknown {
                Ok(ActivationCommitOutcome::DurabilityUnknown(
                    PanelError::commit_outcome_unknown("injected directory sync failure"),
                ))
            } else {
                Ok(ActivationCommitOutcome::Committed)
            }
        }
    }

    fn snapshot(revision: u64) -> RuntimeSnapshot {
        RuntimeSnapshot::empty(RevisionId::new(revision))
    }

    #[derive(Default)]
    struct RecordingEventSink(StdMutex<Vec<GatewayEvent>>);

    impl GatewayEventSink for RecordingEventSink {
        fn emit(&self, event: &GatewayEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    struct PanickingEventSink;

    impl GatewayEventSink for PanickingEventSink {
        fn emit(&self, _event: &GatewayEvent) {
            panic!("injected event sink panic");
        }
    }

    #[tokio::test]
    async fn directly_injected_event_sink_cannot_unwind_from_restore() {
        let engine = DurableGatewayEngine::restore_with_options(
            Arc::new(TestAdapter::new()),
            Arc::new(MemoryStore::default()),
            DurableGatewayEngineOptions::default().with_event_sink(Arc::new(PanickingEventSink)),
        )
        .await
        .expect("event delivery is isolated from runtime restoration");

        assert!(engine.status().await.unwrap().ready);
    }

    #[tokio::test]
    async fn storage_failure_never_switches_the_data_plane() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let events = Arc::new(RecordingEventSink::default());
        let engine = DurableGatewayEngine::restore_with_options(
            Arc::clone(&adapter),
            Arc::clone(&store),
            DurableGatewayEngineOptions::default()
                .with_event_sink(Arc::clone(&events) as Arc<dyn GatewayEventSink>),
        )
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
        assert!(!engine.status().await.unwrap().ready);
        assert!(events.0.lock().unwrap().iter().any(|event| matches!(
            event,
            GatewayEvent::Degraded {
                operation: GatewayOperation::CommitActivation,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn unknown_commit_outcome_reconciles_the_current_namespace() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let events = Arc::new(RecordingEventSink::default());
        let engine = DurableGatewayEngine::restore_with_options(
            Arc::clone(&adapter),
            Arc::clone(&store),
            DurableGatewayEngineOptions::default()
                .with_event_sink(Arc::clone(&events) as Arc<dyn GatewayEventSink>),
        )
        .await
        .unwrap();
        let prepared = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        store.state.lock().await.commit_durability_unknown = true;

        let error = engine
            .activate(ActivateRequest {
                prepare_token: prepared.prepare_token,
                expected_active_hash: None,
            })
            .await
            .unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::COMMIT_OUTCOME_UNKNOWN);
        let published_hash = store
            .state
            .lock()
            .await
            .active
            .as_ref()
            .map(|record| record.envelope.snapshot.content_hash.clone());
        assert_eq!(adapter.active_hash(), published_hash);
        assert_eq!(engine.status().await.unwrap().active_hash, published_hash);
        assert!(!engine.status().await.unwrap().ready);
        assert!(events.0.lock().unwrap().iter().any(|event| matches!(
            event,
            GatewayEvent::Degraded {
                operation: GatewayOperation::CommitActivation,
                error_code,
            } if error_code.as_str() == ErrorCode::COMMIT_OUTCOME_UNKNOWN
        )));
    }

    #[tokio::test]
    async fn prepared_budget_rejects_excess_work_without_persisting_it() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        let engine = DurableGatewayEngine::restore_with_options(
            adapter,
            Arc::clone(&store),
            DurableGatewayEngineOptions::default()
                .with_prepared_policy(Arc::new(PreparedSnapshotBudget::new(1).unwrap())),
        )
        .await
        .unwrap();

        engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let error = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(2),
            })
            .await
            .unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        assert_eq!(store.state.lock().await.prepared.len(), 1);
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

    #[tokio::test]
    async fn request_cancellation_cannot_strand_a_persisted_prepare() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
        store.pause_save.store(true, Ordering::Release);
        let engine = Arc::new(
            DurableGatewayEngine::restore(Arc::clone(&adapter), Arc::clone(&store))
                .await
                .unwrap(),
        );
        let prepare = {
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
        prepare.abort();
        store.continue_save.notify_one();

        for _ in 0..100 {
            if !store.state.lock().await.prepared.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(store.state.lock().await.prepared.len(), 1);
        assert_eq!(engine.status().await.unwrap().prepared_count, 1);
    }

    #[tokio::test]
    async fn request_cancellation_cannot_strand_a_deleted_prepare() {
        let adapter = Arc::new(TestAdapter::new());
        let store = Arc::new(MemoryStore::default());
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
        store.pause_delete.store(true, Ordering::Release);
        let abort = {
            let engine = Arc::clone(&engine);
            let token = prepared.prepare_token.clone();
            tokio::spawn(async move { engine.abort(token).await })
        };
        store.delete_started.notified().await;
        abort.abort();
        store.continue_delete.notify_one();

        for _ in 0..100 {
            if store.state.lock().await.prepared.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(store.state.lock().await.prepared.is_empty());
        assert_eq!(engine.status().await.unwrap().prepared_count, 0);
    }
}
