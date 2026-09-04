#![forbid(unsafe_code)]

use gateway_pingora::PingoraGatewayAdapter;
use panel_domain::RevisionId;
use panel_engine::{ActivateRequest, GatewayEngine, PrepareRequest};
use panel_gateway_runtime::DurableGatewayEngine;
use panel_ir::RuntimeSnapshot;
use snapshot_store_fs::FileSnapshotStore;
use std::{fs, path::PathBuf, sync::Arc};
use uuid::Uuid;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("pingora-panel-stack-{}", Uuid::new_v4())))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn real_composition_restores_the_persisted_pingora_snapshot() {
    let state = TemporaryDirectory::new();
    let first_adapter = Arc::new(PingoraGatewayAdapter::new());
    let first_store = Arc::new(FileSnapshotStore::new(&state.0));
    let first = DurableGatewayEngine::restore(Arc::clone(&first_adapter), first_store)
        .await
        .unwrap();
    let prepared = first
        .prepare(PrepareRequest {
            snapshot: RuntimeSnapshot::empty(RevisionId::new(1)),
        })
        .await
        .unwrap();
    let activated = first
        .activate(ActivateRequest {
            prepare_token: prepared.prepare_token,
            expected_active_hash: None,
        })
        .await
        .unwrap();
    assert_eq!(
        first_adapter.active_snapshot().unwrap().content_hash,
        activated.content_hash
    );
    drop(first);
    drop(first_adapter);

    let restored_adapter = Arc::new(PingoraGatewayAdapter::new());
    let restored_store = Arc::new(FileSnapshotStore::new(&state.0));
    let restored = DurableGatewayEngine::restore(Arc::clone(&restored_adapter), restored_store)
        .await
        .unwrap();
    assert_eq!(
        restored.status().await.unwrap().active_hash,
        Some(activated.content_hash.clone())
    );
    assert_eq!(
        restored_adapter.active_snapshot().unwrap().content_hash,
        activated.content_hash
    );
}
