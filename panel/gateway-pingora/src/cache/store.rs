//! The gateway's cache store (ADR 0043): whole responses in memory, admitted
//! and evicted by TinyUFO by their size.

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use pingora_cache::{
    key::{CacheHashKey, HashBinary},
    storage::{
        HandleHit, HandleMiss, HitHandler, MissFinishType, MissHandler, PurgeOutcome, PurgeTarget,
        PurgeType, Storage,
    },
    trace::SpanHandle,
    CacheKey, CacheMeta,
};
use pingora_core::{Error, ErrorType, Result};
use std::{
    any::Any,
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::SystemTime,
};
use tinyufo::TinyUfo;

/// TinyUFO weighs entries in `u16`, so they weigh in KiB: an entry of up to
/// 64 MiB weighs what it takes.
const WEIGHT_UNIT: u64 = 1 << 10;
/// The most keys purged one by one that are remembered; past it the whole
/// store is emptied instead.
const MOST_PURGED_KEYS: usize = 65_536;

struct Stored {
    /// The combined key, telling apart keys whose hashes collide.
    key: HashBinary,
    primary: HashBinary,
    meta: (Vec<u8>, Vec<u8>),
    body: Bytes,
    stored_at: SystemTime,
}

impl Stored {
    fn bytes(&self) -> u64 {
        (self.body.len() + self.meta.0.len() + self.meta.1.len()) as u64
    }

    fn weight(&self) -> u16 {
        u16::try_from(self.bytes().div_ceil(WEIGHT_UNIT).max(1)).unwrap_or(u16::MAX)
    }
}

/// What the store holds, as statistics report it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StoreUsage {
    pub bytes: u64,
    pub entries: u64,
    pub max_bytes: u64,
}

pub(crate) struct CacheStore {
    objects: ArcSwap<TinyUfo<u64, Arc<Stored>>>,
    max_bytes: AtomicU64,
    bytes: AtomicI64,
    entries: AtomicI64,
    /// When keys were purged, so what was stored for them before is not
    /// served, whichever variant it is.
    purged: Mutex<HashMap<HashBinary, SystemTime>>,
}

fn slot(key: &HashBinary) -> u64 {
    u64::from_le_bytes(key[..8].try_into().expect("keys are 16 bytes"))
}

fn ufo(max_bytes: u64) -> TinyUfo<u64, Arc<Stored>> {
    let units = usize::try_from(max_bytes / WEIGHT_UNIT)
        .unwrap_or(usize::MAX)
        .max(1);
    TinyUfo::new(units, (units / 8).max(1024))
}

impl CacheStore {
    pub(crate) fn new(max_bytes: u64) -> Self {
        Self {
            objects: ArcSwap::from_pointee(ufo(max_bytes)),
            max_bytes: AtomicU64::new(max_bytes),
            bytes: AtomicI64::new(0),
            entries: AtomicI64::new(0),
            purged: Mutex::new(HashMap::new()),
        }
    }

    /// Bounds the store at `max_bytes`; a new size empties it.
    pub(crate) fn resize(&self, max_bytes: u64) {
        if self.max_bytes.swap(max_bytes, Ordering::AcqRel) != max_bytes {
            self.clear();
        }
    }

    /// Drops everything stored.
    pub(crate) fn clear(&self) {
        let limit = self.max_bytes.load(Ordering::Acquire);
        self.objects.store(Arc::new(ufo(limit)));
        self.bytes.store(0, Ordering::Release);
        self.entries.store(0, Ordering::Release);
        self.purged.lock().clear();
    }

    /// Keeps what is stored for `primary` keys now from being served.
    pub(crate) fn purge_keys(&self, primary: impl IntoIterator<Item = HashBinary>) {
        let now = SystemTime::now();
        let mut purged = self.purged.lock();
        purged.extend(primary.into_iter().map(|key| (key, now)));
        if purged.len() > MOST_PURGED_KEYS {
            drop(purged);
            self.clear();
        }
    }

    pub(crate) fn usage(&self) -> StoreUsage {
        StoreUsage {
            bytes: u64::try_from(self.bytes.load(Ordering::Acquire)).unwrap_or(0),
            entries: u64::try_from(self.entries.load(Ordering::Acquire)).unwrap_or(0),
            max_bytes: self.max_bytes.load(Ordering::Acquire),
        }
    }

    fn forget(&self, stored: &Stored) {
        self.bytes.fetch_sub(
            i64::try_from(stored.bytes()).unwrap_or(i64::MAX),
            Ordering::AcqRel,
        );
        self.entries.fetch_sub(1, Ordering::AcqRel);
    }

    fn remove(&self, key: &HashBinary) -> bool {
        let objects = self.objects.load();
        match objects.get(&slot(key)) {
            Some(stored) if stored.key == *key => {
                if let Some(removed) = objects.remove(&slot(key)) {
                    self.forget(&removed);
                }
                true
            }
            _ => false,
        }
    }

    fn insert(&self, stored: Stored) -> usize {
        let objects = self.objects.load();
        let key = slot(&stored.key);
        if let Some(previous) = objects.remove(&key) {
            self.forget(&previous);
        }
        let size = stored.body.len();
        let weight = stored.weight();
        self.bytes.fetch_add(
            i64::try_from(stored.bytes()).unwrap_or(i64::MAX),
            Ordering::AcqRel,
        );
        self.entries.fetch_add(1, Ordering::AcqRel);
        for evicted in objects.put(key, Arc::new(stored), weight) {
            self.forget(&evicted.data);
        }
        size
    }

    fn found(&self, key: &CacheKey) -> Option<Arc<Stored>> {
        let combined = key.combined_bin();
        let stored = self
            .objects
            .load()
            .get(&slot(&combined))
            .filter(|stored| stored.key == combined)?;
        let purged_at = self.purged.lock().get(&stored.primary).copied();
        if purged_at.is_some_and(|at| stored.stored_at <= at) {
            self.remove(&combined);
            return None;
        }
        Some(stored)
    }

    fn replace_meta(&self, stored: &Stored, meta: (Vec<u8>, Vec<u8>)) {
        self.insert(Stored {
            key: stored.key,
            primary: stored.primary,
            meta,
            body: stored.body.clone(),
            stored_at: stored.stored_at,
        });
    }
}

struct Hit {
    body: Bytes,
    done: bool,
    start: usize,
    end: usize,
}

#[async_trait]
impl HandleHit for Hit {
    async fn read_body(&mut self) -> Result<Option<Bytes>> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        Ok(Some(self.body.slice(self.start..self.end)))
    }

    async fn finish(
        self: Box<Self>,
        _storage: &'static (dyn Storage + Sync),
        _key: &CacheKey,
        _trace: &SpanHandle,
    ) -> Result<()> {
        Ok(())
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(&mut self, start: usize, end: Option<usize>) -> Result<()> {
        if start >= self.body.len() {
            return Error::e_explain(
                ErrorType::InternalError,
                format!("seek start out of range {start} >= {}", self.body.len()),
            );
        }
        self.start = start;
        if let Some(end) = end {
            self.end = end.min(self.body.len());
        }
        self.done = false;
        Ok(())
    }

    fn get_eviction_weight(&self) -> usize {
        self.body.len()
    }

    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }

    fn as_any_mut(&mut self) -> &mut (dyn Any + Send + Sync) {
        self
    }
}

struct Miss {
    store: &'static CacheStore,
    key: HashBinary,
    primary: HashBinary,
    meta: (Vec<u8>, Vec<u8>),
    body: BytesMut,
}

#[async_trait]
impl HandleMiss for Miss {
    async fn write_body(&mut self, data: Bytes, _eof: bool) -> Result<()> {
        self.body.extend_from_slice(&data);
        Ok(())
    }

    async fn finish(self: Box<Self>) -> Result<MissFinishType> {
        let size = self.store.insert(Stored {
            key: self.key,
            primary: self.primary,
            meta: self.meta,
            body: self.body.freeze(),
            stored_at: SystemTime::now(),
        });
        Ok(MissFinishType::Created(size))
    }
}

#[async_trait]
impl Storage for CacheStore {
    async fn lookup(
        &'static self,
        key: &CacheKey,
        _trace: &SpanHandle,
    ) -> Result<Option<(CacheMeta, HitHandler)>> {
        let Some(stored) = self.found(key) else {
            return Ok(None);
        };
        let meta = CacheMeta::deserialize(&stored.meta.0, &stored.meta.1)?;
        let hit = Hit {
            end: stored.body.len(),
            body: stored.body.clone(),
            done: false,
            start: 0,
        };
        Ok(Some((meta, Box::new(hit))))
    }

    async fn get_miss_handler(
        &'static self,
        key: &CacheKey,
        meta: &CacheMeta,
        _trace: &SpanHandle,
    ) -> Result<MissHandler> {
        Ok(Box::new(Miss {
            store: self,
            key: key.combined_bin(),
            primary: key.primary_bin(),
            meta: meta.serialize()?,
            body: BytesMut::new(),
        }))
    }

    async fn purge(
        &'static self,
        target: PurgeTarget<'_>,
        _purge_type: PurgeType,
        _trace: &SpanHandle,
    ) -> Result<PurgeOutcome> {
        Ok(if self.remove(&target.key().combined_bin()) {
            PurgeOutcome::Purged(None)
        } else {
            PurgeOutcome::NotFound
        })
    }

    async fn expire(
        &'static self,
        target: PurgeTarget<'_>,
        _trace: &SpanHandle,
    ) -> Result<PurgeOutcome> {
        let key = target.key().combined_bin();
        let Some(stored) = self
            .objects
            .load()
            .get(&slot(&key))
            .filter(|stored| stored.key == key)
        else {
            return Ok(PurgeOutcome::NotFound);
        };
        let mut meta = CacheMeta::deserialize(&stored.meta.0, &stored.meta.1)?;
        meta.expire_at(SystemTime::now());
        self.replace_meta(&stored, meta.serialize()?);
        Ok(PurgeOutcome::Expired)
    }

    async fn update_meta(
        &'static self,
        key: &CacheKey,
        meta: &CacheMeta,
        _trace: &SpanHandle,
    ) -> Result<bool> {
        let Some(stored) = self.found(key) else {
            return Ok(false);
        };
        self.replace_meta(&stored, meta.serialize()?);
        Ok(true)
    }

    fn as_any(&self) -> &(dyn Any + Send + Sync + 'static) {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_cache::trace::Span;
    use pingora_http::ResponseHeader;
    use std::time::Duration;

    fn meta() -> CacheMeta {
        let now = SystemTime::now();
        let header = ResponseHeader::build(200, None).unwrap();
        CacheMeta::new(now + Duration::from_secs(60), now, 0, 0, header)
    }

    fn leaked(max_bytes: u64) -> &'static CacheStore {
        Box::leak(Box::new(CacheStore::new(max_bytes)))
    }

    async fn put(store: &'static CacheStore, key: &CacheKey, body: &'static [u8]) {
        let trace = Span::inactive().handle();
        let mut miss = store.get_miss_handler(key, &meta(), &trace).await.unwrap();
        miss.write_body(Bytes::from_static(body), true)
            .await
            .unwrap();
        miss.finish().await.unwrap();
    }

    async fn read(store: &'static CacheStore, key: &CacheKey) -> Option<Bytes> {
        let (_, mut hit) = store
            .lookup(key, &Span::inactive().handle())
            .await
            .unwrap()?;
        hit.read_body().await.unwrap()
    }

    #[tokio::test]
    async fn responses_are_stored_counted_and_purged() {
        let store = leaked(1 << 20);
        let key = CacheKey::new("https://shop.example/", "");
        assert!(read(store, &key).await.is_none());
        put(store, &key, b"page").await;
        assert_eq!(read(store, &key).await.unwrap(), "page");
        let usage = store.usage();
        assert_eq!(usage.entries, 1);
        assert!(usage.bytes >= 4);
        put(store, &key, b"again").await;
        assert_eq!(store.usage().entries, 1, "a key is stored once");

        store.purge_keys([key.primary_bin()]);
        assert!(
            read(store, &key).await.is_none(),
            "a purged key is not served"
        );
        assert_eq!(store.usage().entries, 0);
        put(store, &key, b"fresh").await;
        assert_eq!(read(store, &key).await.unwrap(), "fresh");

        store.resize(2 << 20);
        assert!(
            read(store, &key).await.is_none(),
            "a new size empties the store"
        );
        assert_eq!(
            store.usage(),
            StoreUsage {
                bytes: 0,
                entries: 0,
                max_bytes: 2 << 20
            }
        );
    }

    #[tokio::test]
    async fn ranges_read_from_the_stored_body() {
        let store = leaked(1 << 20);
        let key = CacheKey::new("https://shop.example/range", "");
        put(store, &key, b"0123456789").await;
        let (_, mut hit) = store
            .lookup(&key, &Span::inactive().handle())
            .await
            .unwrap()
            .unwrap();
        hit.seek(2, Some(5)).unwrap();
        assert_eq!(hit.read_body().await.unwrap().unwrap(), "234");
        assert!(hit.read_body().await.unwrap().is_none());
        assert!(hit.seek(10, None).is_err());
    }

    #[tokio::test]
    async fn expiring_keeps_the_body_for_revalidation() {
        let store = leaked(1 << 20);
        let key = CacheKey::new("https://shop.example/stale", "");
        put(store, &key, b"body").await;
        let outcome = store
            .expire(
                PurgeTarget::Active(&key.to_compact()),
                &Span::inactive().handle(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, PurgeOutcome::Expired);
        let (meta, _) = store
            .lookup(&key, &Span::inactive().handle())
            .await
            .unwrap()
            .unwrap();
        assert!(!meta.is_fresh(SystemTime::now()));
        assert!(store
            .update_meta(&key, &self::meta(), &Span::inactive().handle())
            .await
            .unwrap());
        let (meta, _) = store
            .lookup(&key, &Span::inactive().handle())
            .await
            .unwrap()
            .unwrap();
        assert!(meta.is_fresh(SystemTime::now()));
    }

    #[tokio::test]
    async fn the_store_stays_within_its_size() {
        let store = leaked(64 << 10);
        static BODY: [u8; 8 << 10] = [7; 8 << 10];
        for index in 0..64 {
            let key = CacheKey::new(format!("https://shop.example/{index}"), "");
            put(store, &key, &BODY).await;
        }
        let usage = store.usage();
        assert!(usage.bytes <= 64 << 10, "{usage:?}");
        assert!(usage.entries < 64, "{usage:?}");
    }
}
