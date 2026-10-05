//! `ngx.shared` dictionaries: one store for every VM of the process, kept
//! across activations while a dictionary keeps its name and capacity.
//!
//! Items are evicted least recently used first when a `set` needs room, and
//! expire after their time to live, as lua-nginx-module's dictionaries do.

use crate::program::SharedDict;
use lru::LruCache;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

/// What a key holds besides a list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Scalar {
    Boolean(bool),
    Number(f64),
    String(Vec<u8>),
}

impl Scalar {
    fn size(&self) -> usize {
        match self {
            Scalar::String(bytes) => bytes.len(),
            _ => 8,
        }
    }
}

#[derive(Clone, Debug)]
enum Item {
    Scalar { value: Scalar, flags: u32 },
    List(VecDeque<Scalar>),
}

impl Item {
    fn size(&self) -> usize {
        match self {
            Item::Scalar { value, .. } => value.size(),
            Item::List(items) => items.iter().map(|item| item.size() + 16).sum(),
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    item: Item,
    expires: Option<Instant>,
}

impl Entry {
    fn expired(&self, now: Instant) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }
}

/// Bytes an entry costs besides its key and value, as lua-nginx-module's
/// slab nodes do.
const ENTRY_OVERHEAD: usize = 64;

fn cost(key: &[u8], item: &Item) -> usize {
    key.len() + item.size() + ENTRY_OVERHEAD
}

/// Why a dictionary operation did not happen, in lua-nginx-module's words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    NoMemory,
    Exists,
    NotFound,
    NotANumber,
    NotAList,
    IsAList,
}

impl Refusal {
    pub const fn message(self) -> &'static str {
        match self {
            Refusal::NoMemory => "no memory",
            Refusal::Exists => "exists",
            Refusal::NotFound => "not found",
            Refusal::NotANumber => "not a number",
            Refusal::NotAList => "value not a list",
            Refusal::IsAList => "value is a list",
        }
    }
}

/// How a `set` treats items in its way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SetMode {
    /// Store, evicting other items for room.
    Set,
    /// Store without evicting unexpired items.
    SafeSet,
    /// Store only if the key holds nothing unexpired.
    Add,
    SafeAdd,
    /// Store only if the key holds something unexpired.
    Replace,
}

/// One dictionary.
#[derive(Debug)]
pub(crate) struct Dict {
    capacity: usize,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    entries: LruCache<Vec<u8>, Entry>,
    used: usize,
}

/// A stored value as `get` returns it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Got {
    pub value: Scalar,
    pub flags: u32,
    pub stale: bool,
}

impl Dict {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Mutex::new(Inner {
                entries: LruCache::unbounded(),
                used: 0,
            }),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn free_space(&self) -> usize {
        self.capacity.saturating_sub(self.inner.lock().used)
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Got>, Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let expired = match inner.entries.get(key) {
            None => return Ok(None),
            Some(entry) if entry.expired(now) => true,
            Some(entry) => {
                return match &entry.item {
                    Item::Scalar { value, flags } => Ok(Some(Got {
                        value: value.clone(),
                        flags: *flags,
                        stale: false,
                    })),
                    Item::List(_) => Err(Refusal::IsAList),
                };
            }
        };
        if expired {
            inner.remove(key);
        }
        Ok(None)
    }

    /// The value even if it has expired, until it is evicted.
    pub fn get_stale(&self, key: &[u8]) -> Result<Option<Got>, Refusal> {
        let now = Instant::now();
        let inner = self.inner.lock();
        match inner.entries.peek(key) {
            None => Ok(None),
            Some(Entry {
                item: Item::Scalar { value, flags },
                expires,
            }) => Ok(Some(Got {
                value: value.clone(),
                flags: *flags,
                stale: expires.is_some_and(|expires| expires <= now),
            })),
            Some(_) => Err(Refusal::IsAList),
        }
    }

    /// Stores `value` for `ttl` (none for ever). Returns whether unexpired
    /// items were evicted to make room.
    pub fn set(
        &self,
        key: &[u8],
        value: Option<Scalar>,
        ttl: Option<Duration>,
        flags: u32,
        mode: SetMode,
    ) -> Result<bool, Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let live = inner
            .entries
            .peek(key)
            .is_some_and(|entry| !entry.expired(now));
        match mode {
            SetMode::Add | SetMode::SafeAdd if live => return Err(Refusal::Exists),
            SetMode::Replace if !live => return Err(Refusal::NotFound),
            _ => {}
        }
        inner.remove(key);
        let Some(value) = value else {
            return Ok(false);
        };
        let item = Item::Scalar { value, flags };
        let evict = matches!(mode, SetMode::Set | SetMode::Add | SetMode::Replace);
        let forcible = inner.make_room(cost(key, &item), self.capacity, evict, now)?;
        inner.insert(key.to_vec(), item, ttl.map(|ttl| now + ttl));
        Ok(forcible)
    }

    pub fn delete(&self, key: &[u8]) {
        self.inner.lock().remove(key);
    }

    /// Adds `by` to the number at `key`, starting from `init` when there is
    /// none. Returns the new value and whether items were evicted.
    pub fn incr(
        &self,
        key: &[u8],
        by: f64,
        init: Option<f64>,
        init_ttl: Option<Duration>,
    ) -> Result<(f64, bool), Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let current = match inner.entries.get_mut(key) {
            Some(entry) if !entry.expired(now) => match &mut entry.item {
                Item::Scalar {
                    value: Scalar::Number(number),
                    ..
                } => {
                    *number += by;
                    return Ok((*number, false));
                }
                Item::Scalar { .. } => return Err(Refusal::NotANumber),
                Item::List(_) => return Err(Refusal::IsAList),
            },
            _ => None::<f64>,
        };
        debug_assert!(current.is_none());
        let Some(init) = init else {
            return Err(Refusal::NotFound);
        };
        inner.remove(key);
        let value = init + by;
        let item = Item::Scalar {
            value: Scalar::Number(value),
            flags: 0,
        };
        let forcible = inner.make_room(cost(key, &item), self.capacity, true, now)?;
        inner.insert(key.to_vec(), item, init_ttl.map(|ttl| now + ttl));
        Ok((value, forcible))
    }

    /// Pushes onto the list at `key`, at its head or tail. Returns its length.
    pub fn push(&self, key: &[u8], value: Scalar, head: bool) -> Result<usize, Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let added = value.size() + 16;
        let live_list = match inner.entries.peek(key) {
            Some(entry) if !entry.expired(now) => match entry.item {
                Item::List(_) => true,
                Item::Scalar { .. } => return Err(Refusal::NotAList),
            },
            _ => false,
        };
        if !live_list {
            inner.remove(key);
            let item = Item::List(VecDeque::new());
            inner.make_room(cost(key, &item) + added, self.capacity, false, now)?;
            inner.insert(key.to_vec(), item, None);
        } else if inner.used + added > self.capacity {
            inner.make_room(added, self.capacity, false, now)?;
        }
        inner.used += added;
        let Some(Entry {
            item: Item::List(items),
            ..
        }) = inner.entries.get_mut(key)
        else {
            return Err(Refusal::NotAList);
        };
        if head {
            items.push_front(value);
        } else {
            items.push_back(value);
        }
        Ok(items.len())
    }

    /// Pops from the list at `key`, from its head or tail.
    pub fn pop(&self, key: &[u8], head: bool) -> Result<Option<Scalar>, Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let (value, empty) = match inner.entries.get_mut(key) {
            None => return Ok(None),
            Some(entry) if entry.expired(now) => (None, true),
            Some(Entry {
                item: Item::List(items),
                ..
            }) => {
                let value = if head {
                    items.pop_front()
                } else {
                    items.pop_back()
                };
                (value, items.is_empty())
            }
            Some(_) => return Err(Refusal::NotAList),
        };
        if let Some(value) = &value {
            inner.used = inner.used.saturating_sub(value.size() + 16);
        }
        if empty {
            inner.remove(key);
        }
        Ok(value)
    }

    pub fn len(&self, key: &[u8]) -> Result<usize, Refusal> {
        let now = Instant::now();
        let inner = self.inner.lock();
        match inner.entries.peek(key) {
            Some(entry) if !entry.expired(now) => match &entry.item {
                Item::List(items) => Ok(items.len()),
                Item::Scalar { .. } => Err(Refusal::NotAList),
            },
            _ => Ok(0),
        }
    }

    /// Time left before `key` expires; zero for an item kept for ever.
    pub fn ttl(&self, key: &[u8]) -> Result<Duration, Refusal> {
        let now = Instant::now();
        let inner = self.inner.lock();
        match inner.entries.peek(key) {
            Some(entry) if !entry.expired(now) => Ok(entry
                .expires
                .map_or(Duration::ZERO, |expires| expires - now)),
            _ => Err(Refusal::NotFound),
        }
    }

    pub fn expire(&self, key: &[u8], ttl: Option<Duration>) -> Result<(), Refusal> {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        match inner.entries.peek_mut(key) {
            Some(entry) if !entry.expired(now) => {
                entry.expires = ttl.map(|ttl| now + ttl);
                Ok(())
            }
            _ => Err(Refusal::NotFound),
        }
    }

    pub fn flush_all(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.used = 0;
    }

    /// Removes up to `max` expired items, every one when `max` is zero.
    pub fn flush_expired(&self, max: usize) -> usize {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        let expired: Vec<Vec<u8>> = inner
            .entries
            .iter()
            .filter(|(_, entry)| entry.expired(now))
            .map(|(key, _)| key.clone())
            .take(if max == 0 { usize::MAX } else { max })
            .collect();
        for key in &expired {
            inner.remove(key);
        }
        expired.len()
    }

    /// Keys of unexpired items, up to `max`, every one when `max` is zero.
    pub fn keys(&self, max: usize) -> Vec<Vec<u8>> {
        let now = Instant::now();
        self.inner
            .lock()
            .entries
            .iter()
            .filter(|(_, entry)| !entry.expired(now))
            .map(|(key, _)| key.clone())
            .take(if max == 0 { usize::MAX } else { max })
            .collect()
    }
}

impl Inner {
    fn remove(&mut self, key: &[u8]) {
        if let Some(entry) = self.entries.pop(key) {
            self.used = self.used.saturating_sub(cost(key, &entry.item));
        }
    }

    fn insert(&mut self, key: Vec<u8>, item: Item, expires: Option<Instant>) {
        self.used += cost(&key, &item);
        self.entries.put(key, Entry { item, expires });
    }

    /// Frees `needed` bytes: expired items first, then the least recently
    /// used when `evict`. Returns whether unexpired items were evicted.
    fn make_room(
        &mut self,
        needed: usize,
        capacity: usize,
        evict: bool,
        now: Instant,
    ) -> Result<bool, Refusal> {
        if needed > capacity {
            return Err(Refusal::NoMemory);
        }
        if self.used + needed <= capacity {
            return Ok(false);
        }
        let expired: Vec<Vec<u8>> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expired(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &expired {
            self.remove(key);
        }
        let mut forcible = false;
        while self.used + needed > capacity {
            if !evict {
                return Err(Refusal::NoMemory);
            }
            let Some((key, entry)) = self.entries.pop_lru() else {
                return Err(Refusal::NoMemory);
            };
            self.used = self.used.saturating_sub(cost(&key, &entry.item));
            forcible = true;
        }
        Ok(forcible)
    }
}

/// The dictionaries of the process, shared by every runtime it starts.
#[derive(Debug, Default)]
pub struct SharedStore {
    dicts: Mutex<HashMap<String, Arc<Dict>>>,
}

impl SharedStore {
    /// The dictionaries `declared` names. One that keeps its capacity keeps
    /// its contents; the others start empty, and undeclared ones are dropped.
    pub(crate) fn resolve(&self, declared: &[SharedDict]) -> HashMap<String, Arc<Dict>> {
        let mut dicts = self.dicts.lock();
        let resolved: HashMap<String, Arc<Dict>> = declared
            .iter()
            .map(|dict| {
                let kept = dicts
                    .get(&dict.name)
                    .filter(|existing| existing.capacity == dict.capacity)
                    .cloned();
                let dict_handle = kept.unwrap_or_else(|| Arc::new(Dict::new(dict.capacity)));
                (dict.name.clone(), dict_handle)
            })
            .collect();
        *dicts = resolved.clone();
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Option<Scalar> {
        Some(Scalar::String(value.as_bytes().to_vec()))
    }

    #[test]
    fn sets_evict_the_least_recently_used_and_safe_sets_do_not() {
        let dict = Dict::new(3 * (ENTRY_OVERHEAD + 2));
        for key in ["a", "b", "c"] {
            assert!(!dict
                .set(key.as_bytes(), text("1"), None, 0, SetMode::Set)
                .unwrap());
        }
        dict.get(b"a").unwrap();
        assert_eq!(
            dict.set(b"d", text("1"), None, 0, SetMode::SafeSet),
            Err(Refusal::NoMemory)
        );
        assert!(dict.set(b"d", text("1"), None, 0, SetMode::Set).unwrap());
        assert!(dict.get(b"b").unwrap().is_none());
        assert!(dict.get(b"a").unwrap().is_some());
    }

    #[test]
    fn adds_replaces_and_increments_follow_their_conditions() {
        let dict = Dict::new(4096);
        dict.set(b"k", text("v"), None, 7, SetMode::Add).unwrap();
        assert_eq!(
            dict.set(b"k", text("v"), None, 0, SetMode::Add),
            Err(Refusal::Exists)
        );
        assert_eq!(
            dict.set(b"x", text("v"), None, 0, SetMode::Replace),
            Err(Refusal::NotFound)
        );
        assert_eq!(dict.get(b"k").unwrap().unwrap().flags, 7);
        assert_eq!(dict.incr(b"n", 1.0, None, None), Err(Refusal::NotFound));
        assert_eq!(dict.incr(b"n", 2.0, Some(10.0), None).unwrap().0, 12.0);
        assert_eq!(dict.incr(b"n", -1.0, None, None).unwrap().0, 11.0);
        assert_eq!(dict.incr(b"k", 1.0, None, None), Err(Refusal::NotANumber));
    }

    #[test]
    fn items_expire_and_stay_readable_as_stale() {
        let dict = Dict::new(4096);
        dict.set(b"k", text("v"), Some(Duration::ZERO), 0, SetMode::Set)
            .unwrap();
        assert!(dict.get_stale(b"k").unwrap().unwrap().stale);
        assert!(dict.get(b"k").unwrap().is_none());
        assert_eq!(dict.ttl(b"k"), Err(Refusal::NotFound));
    }

    #[test]
    fn lists_push_pop_and_refuse_scalars() {
        let dict = Dict::new(4096);
        assert_eq!(dict.push(b"q", Scalar::Number(1.0), false).unwrap(), 1);
        assert_eq!(dict.push(b"q", Scalar::Number(2.0), true).unwrap(), 2);
        assert_eq!(dict.pop(b"q", true).unwrap(), Some(Scalar::Number(2.0)));
        assert_eq!(dict.len(b"q").unwrap(), 1);
        assert_eq!(dict.get(b"q"), Err(Refusal::IsAList));
        dict.set(b"s", text("v"), None, 0, SetMode::Set).unwrap();
        assert_eq!(
            dict.push(b"s", Scalar::Number(1.0), true),
            Err(Refusal::NotAList)
        );
    }

    #[test]
    fn the_store_keeps_dictionaries_whose_capacity_stays() {
        let store = SharedStore::default();
        let first = store.resolve(&[SharedDict {
            name: "a".into(),
            capacity: 4096,
        }]);
        first["a"]
            .set(b"k", text("v"), None, 0, SetMode::Set)
            .unwrap();
        let kept = store.resolve(&[SharedDict {
            name: "a".into(),
            capacity: 4096,
        }]);
        assert!(kept["a"].get(b"k").unwrap().is_some());
        let resized = store.resolve(&[SharedDict {
            name: "a".into(),
            capacity: 8192,
        }]);
        assert!(resized["a"].get(b"k").unwrap().is_none());
    }
}
