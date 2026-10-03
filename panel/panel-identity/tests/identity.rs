#![forbid(unsafe_code)]

use async_trait::async_trait;
use panel_identity::{
    conformance::{check, StoreUnderTest},
    memory::{MemoryIdentityStore, RecordedEvent},
    IdentityStore,
};
use std::sync::Arc;

struct Memory(Arc<MemoryIdentityStore>);

#[async_trait]
impl StoreUnderTest for Memory {
    fn store(&self) -> Arc<dyn IdentityStore> {
        self.0.clone()
    }

    async fn events(&self) -> Vec<RecordedEvent> {
        self.0.events()
    }
}

#[tokio::test]
async fn the_memory_store_follows_the_identity_rules() {
    check(|| async { Memory(Arc::new(MemoryIdentityStore::default())) }).await;
}
