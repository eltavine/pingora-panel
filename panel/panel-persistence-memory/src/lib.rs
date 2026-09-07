#![forbid(unsafe_code)]

//! Concurrency-safe in-memory application persistence.
//!
//! This adapter is intentionally independent from `panel-application`'s
//! workflow. It is useful for local composition, black-box tests and ephemeral
//! deployments; a durable SQL adapter can replace it without changing callers.

use async_trait::async_trait;
use panel_application::{
    IdempotencyClaim, IdempotencyKey, IdempotencyLookup, IdempotencyRecord, IdempotencyRepository,
};
use panel_domain::ContentHash;
use panel_errors::{PanelError, Result};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct MemoryIdempotencyRepository {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

#[derive(Clone)]
enum Entry {
    InProgress { request_hash: ContentHash },
    Completed(IdempotencyRecord),
}

impl MemoryIdempotencyRepository {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.entries.lock().await.is_empty()
    }
}

#[async_trait]
impl IdempotencyRepository for MemoryIdempotencyRepository {
    async fn claim(
        &self,
        key: &IdempotencyKey,
        request_hash: &ContentHash,
    ) -> Result<IdempotencyClaim> {
        let mut entries = self.entries.lock().await;
        match entries.get(key.as_str()) {
            None => {
                entries.insert(
                    key.as_str().to_owned(),
                    Entry::InProgress {
                        request_hash: request_hash.clone(),
                    },
                );
                Ok(IdempotencyClaim::Acquired)
            }
            Some(Entry::InProgress {
                request_hash: stored_hash,
            }) if stored_hash == request_hash => Ok(IdempotencyClaim::InProgress),
            Some(Entry::InProgress { .. }) => Ok(IdempotencyClaim::Conflict),
            Some(Entry::Completed(record)) if record.request_hash() == request_hash => {
                Ok(IdempotencyClaim::Replay(record.clone()))
            }
            Some(Entry::Completed(_)) => Ok(IdempotencyClaim::Conflict),
        }
    }

    async fn complete(&self, key: &IdempotencyKey, record: IdempotencyRecord) -> Result<()> {
        let mut entries = self.entries.lock().await;
        match entries.get(key.as_str()) {
            Some(Entry::InProgress { request_hash }) if request_hash == record.request_hash() => {
                entries.insert(key.as_str().to_owned(), Entry::Completed(record));
                Ok(())
            }
            Some(Entry::Completed(existing)) if existing == &record => Ok(()),
            Some(_) => Err(PanelError::conflict(
                "idempotency receipt does not match the active claim",
            )),
            None => Err(PanelError::not_found("idempotency claim not found")),
        }
    }

    async fn abort(&self, key: &IdempotencyKey, request_hash: &ContentHash) -> Result<()> {
        let mut entries = self.entries.lock().await;
        if matches!(
            entries.get(key.as_str()),
            Some(Entry::InProgress { request_hash: stored }) if stored == request_hash
        ) {
            entries.remove(key.as_str());
        }
        Ok(())
    }

    async fn lookup(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        let entries = self.entries.lock().await;
        Ok(match entries.get(key.as_str()) {
            None => IdempotencyLookup::Missing,
            Some(Entry::InProgress { .. }) => IdempotencyLookup::InProgress,
            Some(Entry::Completed(record)) => IdempotencyLookup::Completed(record.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_application::{ActivatedDeployment, DeploymentOutcome};
    use panel_domain::RevisionId;

    fn key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value).unwrap()
    }

    fn hash(value: &[u8]) -> ContentHash {
        ContentHash::from_bytes(value)
    }

    fn record(request_hash: ContentHash) -> IdempotencyRecord {
        IdempotencyRecord::new(
            request_hash,
            DeploymentOutcome::Succeeded(ActivatedDeployment::new(
                RevisionId::new(1),
                hash(b"active"),
                None,
            )),
        )
    }

    #[tokio::test]
    async fn claim_complete_and_lookup_are_linearizable() {
        let repository = MemoryIdempotencyRepository::new();
        let key = key("idem-1");
        let request_hash = hash(b"request");
        assert_eq!(
            repository.claim(&key, &request_hash).await.unwrap(),
            IdempotencyClaim::Acquired
        );
        assert_eq!(
            repository.lookup(&key).await.unwrap(),
            IdempotencyLookup::InProgress
        );
        let receipt = record(request_hash.clone());
        repository.complete(&key, receipt.clone()).await.unwrap();
        assert_eq!(
            repository.lookup(&key).await.unwrap(),
            IdempotencyLookup::Completed(receipt)
        );
        assert_eq!(repository.len().await, 1);
    }

    #[tokio::test]
    async fn conflicting_reuse_never_replays_or_replaces_receipt() {
        let repository = MemoryIdempotencyRepository::new();
        let key = key("idem-2");
        let first = hash(b"first");
        repository.claim(&key, &first).await.unwrap();
        assert_eq!(
            repository.claim(&key, &hash(b"second")).await.unwrap(),
            IdempotencyClaim::Conflict
        );
    }
}
