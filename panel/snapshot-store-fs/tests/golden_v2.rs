#![forbid(unsafe_code)]

use panel_domain::{ContentHash, RevisionId};
use panel_engine::SnapshotStore;
use panel_errors::ErrorCode;
use snapshot_store_fs::{
    FileSnapshotStore, JsonSnapshotRecordCodecV1, JsonSnapshotRecordCodecV2,
    SnapshotRecordCodecRegistry,
};
use std::{fs, path::PathBuf, sync::Arc};
use uuid::Uuid;

const GOLDEN_V1_ACTIVE: &[u8] = include_bytes!("fixtures/v1/active.json");
const GOLDEN_V2_ACTIVE: &[u8] = include_bytes!("fixtures/v2/active.json");
const GOLDEN_V2_BYTES_HASH: &str =
    "d1f941d00403ccb2ce98db9057cd443b5e87a0581f6b2d3b0542264dd47c7b17";

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pingora-panel-golden-v2-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write_active(&self, bytes: &[u8]) {
        fs::write(self.0.join("active.json"), bytes).unwrap();
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn v2_registry() -> Arc<SnapshotRecordCodecRegistry> {
    Arc::new(
        SnapshotRecordCodecRegistry::new(Arc::new(JsonSnapshotRecordCodecV2))
            .unwrap()
            .with_reader(Arc::new(JsonSnapshotRecordCodecV1))
            .unwrap(),
    )
}

#[test]
fn committed_v2_fixture_bytes_are_immutable() {
    assert_eq!(
        ContentHash::from_bytes(GOLDEN_V2_ACTIVE).as_str(),
        GOLDEN_V2_BYTES_HASH
    );
}

#[tokio::test]
async fn committed_v2_fixture_is_readable_only_with_an_explicit_v2_reader() {
    let temporary = TemporaryDirectory::new();
    temporary.write_active(GOLDEN_V2_ACTIVE);

    let v1_only = FileSnapshotStore::new(&temporary.0);
    let error = v1_only.load_active().await.unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);

    let compatible = FileSnapshotStore::with_codec_registry(&temporary.0, v2_registry());
    let active = compatible.load_active().await.unwrap().unwrap();
    assert_eq!(active.envelope.snapshot.revision_id, RevisionId::new(41));
    assert_eq!(active.receipt.adapter_version, "golden-adapter-v2");
}

#[tokio::test]
async fn reopening_v1_with_a_v2_writer_migrates_on_the_next_commit() {
    let temporary = TemporaryDirectory::new();
    temporary.write_active(GOLDEN_V1_ACTIVE);
    let store = FileSnapshotStore::with_codec_registry(&temporary.0, v2_registry());

    let active = store.load_active().await.unwrap().unwrap();
    store.commit_activation(active.clone()).await.unwrap();
    drop(store);

    let encoded: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.0.join("active.json")).unwrap()).unwrap();
    assert_eq!(encoded["format_version"], 2);

    let reopened = FileSnapshotStore::with_codec_registry(&temporary.0, v2_registry());
    assert_eq!(reopened.load_active().await.unwrap(), Some(active));
}
