#![forbid(unsafe_code)]

use panel_domain::{ContentHash, RevisionId};
use panel_engine::SnapshotStore;
use panel_errors::ErrorCode;
use snapshot_store_fs::{FileSnapshotStore, SnapshotRecordCodecRegistry};
use std::{fs, path::PathBuf};
use uuid::Uuid;

const GOLDEN_ACTIVE: &[u8] = include_bytes!("fixtures/v1/active.json");
const GOLDEN_POPULATED_ACTIVE: &[u8] = include_bytes!("fixtures/v1/populated-active.json");
const GOLDEN_HASH: &str = "766d68b0accced7c5d5835cc6f988fb69fe7eb80ae5847045943d1ab8dcc7dd6";
const GOLDEN_POPULATED_HASH: &str =
    "468572ac2602868989cb7ca5fcc424d7744f2980d8a92b91694d585b03d9443f";
const GOLDEN_ACTIVE_BYTES_HASH: &str =
    "aa9bae170a50088a823f5a6dc07714221b12d459a5d7f6e7c1a278232d994173";
const GOLDEN_POPULATED_ACTIVE_BYTES_HASH: &str =
    "c0947262b28f45e9e94277224f052cdf6724b045e9d9c2a0b554058d1fe010a9";

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

#[test]
fn committed_v1_fixture_bytes_are_immutable() {
    assert_eq!(
        ContentHash::from_bytes(GOLDEN_ACTIVE).as_str(),
        GOLDEN_ACTIVE_BYTES_HASH
    );
    assert_eq!(
        ContentHash::from_bytes(GOLDEN_POPULATED_ACTIVE).as_str(),
        GOLDEN_POPULATED_ACTIVE_BYTES_HASH
    );
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
async fn populated_v1_fixture_locks_nested_ir_and_canonical_hash() {
    let temporary = TemporaryDirectory::new();
    temporary.write_active(GOLDEN_POPULATED_ACTIVE);
    let store = FileSnapshotStore::new(&temporary.0);

    let active = store.load_active().await.unwrap().unwrap();
    let snapshot = &active.envelope.snapshot;
    assert_eq!(snapshot.content_hash.as_str(), GOLDEN_POPULATED_HASH);
    assert_eq!(snapshot.content_hash(), snapshot.content_hash);
    assert_eq!(snapshot.listeners.len(), 1);
    assert_eq!(snapshot.sites[0].id.as_str(), "site-main");
    assert_eq!(snapshot.routes[0].id.as_str(), "route-api");
    assert_eq!(snapshot.upstream_pools[0].id.as_str(), "pool-api");
    assert_eq!(snapshot.upstream_pools[0].endpoints[0].weight, 10);
    assert_eq!(snapshot.tls_profiles[0].id, "tls-main");
    assert_eq!(snapshot.header_policies[0].id, "headers-main");
    assert_eq!(snapshot.static_content[0].id, "static-main");
    assert_eq!(snapshot.cache_policies[0].id, "cache-api");
    assert_eq!(snapshot.security_policies[0].id, "security-api");
    assert_eq!(snapshot.lua_policies[0].id, "lua-auth");

    let reencoded = SnapshotRecordCodecRegistry::default()
        .encode(&active)
        .unwrap();
    let fixture_value: serde_json::Value = serde_json::from_slice(GOLDEN_POPULATED_ACTIVE).unwrap();
    let reencoded_value: serde_json::Value = serde_json::from_slice(&reencoded).unwrap();
    assert_eq!(reencoded_value, fixture_value);
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
