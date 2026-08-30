use panel_domain::RevisionId;
use panel_engine::SnapshotStore;
use panel_errors::ErrorCode;
use snapshot_store_fs::FileSnapshotStore;
use std::{fs, path::PathBuf};
use uuid::Uuid;

const GOLDEN_ACTIVE: &[u8] = include_bytes!("fixtures/v1/active.json");
const GOLDEN_HASH: &str = "766d68b0accced7c5d5835cc6f988fb69fe7eb80ae5847045943d1ab8dcc7dd6";

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pingora-panel-golden-v1-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write_active(&self, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("active.json");
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn committed_v1_fixture_remains_readable() {
    let temporary = TemporaryDirectory::new();
    temporary.write_active(GOLDEN_ACTIVE);
    let store = FileSnapshotStore::new(&temporary.0);

    let active = store.load_active().await.unwrap().unwrap();
    assert_eq!(active.envelope.snapshot.revision_id, RevisionId::new(41));
    assert_eq!(active.envelope.snapshot.content_hash.as_str(), GOLDEN_HASH);
    assert_eq!(active.receipt.content_hash.as_str(), GOLDEN_HASH);
    assert_eq!(active.receipt.adapter_version, "golden-adapter-v1");
    assert_eq!(
        active.receipt.prepare_token.as_str(),
        "golden-prepare-token-v1"
    );
}

#[tokio::test]
async fn unknown_store_version_fails_without_touching_the_fixture() {
    let temporary = TemporaryDirectory::new();
    let incompatible = String::from_utf8(GOLDEN_ACTIVE.to_vec())
        .unwrap()
        .replace("\"format_version\": 1", "\"format_version\": 999");
    let active_path = temporary.write_active(incompatible.as_bytes());
    let store = FileSnapshotStore::new(&temporary.0);

    let error = store.load_active().await.unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    assert_eq!(fs::read(active_path).unwrap(), incompatible.as_bytes());
}

#[tokio::test]
async fn truncated_or_hash_mismatched_v1_state_is_quarantined_logically() {
    let truncated_directory = TemporaryDirectory::new();
    let truncated_path = truncated_directory.write_active(&GOLDEN_ACTIVE[..64]);
    let truncated_store = FileSnapshotStore::new(&truncated_directory.0);
    let truncated = truncated_store.load_active().await.unwrap_err();
    assert_eq!(truncated.code.as_str(), ErrorCode::CORRUPT_STATE);
    assert!(truncated_path.exists());

    let mismatched_directory = TemporaryDirectory::new();
    let mismatched = String::from_utf8(GOLDEN_ACTIVE.to_vec())
        .unwrap()
        .replace(GOLDEN_HASH, &"0".repeat(64));
    let mismatched_path = mismatched_directory.write_active(mismatched.as_bytes());
    let mismatched_store = FileSnapshotStore::new(&mismatched_directory.0);
    let mismatch = mismatched_store.load_active().await.unwrap_err();
    assert_eq!(mismatch.code.as_str(), ErrorCode::CORRUPT_STATE);
    assert!(mismatched_path.exists());
}

#[tokio::test]
async fn abandoned_temporary_files_do_not_change_the_active_fixture() {
    let temporary = TemporaryDirectory::new();
    temporary.write_active(GOLDEN_ACTIVE);
    fs::write(temporary.0.join(".snapshot-abandoned.tmp"), b"partial").unwrap();
    let store = FileSnapshotStore::new(&temporary.0);

    let active = store.load_active().await.unwrap().unwrap();
    assert_eq!(active.envelope.snapshot.content_hash.as_str(), GOLDEN_HASH);
}
