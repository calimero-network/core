use core::fmt::Debug;
use std::collections::btree_map::IntoIter;
use std::collections::BTreeMap;

use tracing::debug;

use calimero_primitives::reflect::Reflect;

pub type Key = Vec<u8>;
pub type Value = Vec<u8>;

pub trait Storage: Reflect {
    fn get(&self, key: &Key) -> Option<Value>;
    fn set(&mut self, key: Key, value: Value) -> Option<Value>;
    fn remove(&mut self, key: &Key) -> Option<Vec<u8>>;
    fn has(&self, key: &Key) -> bool;

    // === Ordered secondary index (SortedMap, core#2559) ===
    //
    // A separate, byte-ordered keyspace (the backend keeps keys in sorted
    // order, so a range scan is a native seek). Keys are the unhashed
    // `collection ‖ order_key`; values are the entry's 32-byte id. This is the
    // node-local, non-synced index that lets `SortedMap` answer range/prefix/
    // page queries without scanning every entry.
    //
    // Default impls make the index inert: a backend that doesn't provide an
    // ordered keyspace leaves these alone and `SortedMap` falls back to its
    // in-memory sort (the storage adaptor gates on `index_supported()`).

    /// Insert/overwrite `key -> value` in the ordered index. Returns whether the
    /// write was persisted, so the collection can avoid stamping its validity
    /// marker (and force a rebuild instead) after a failed write.
    fn index_set(&mut self, key: &[u8], value: &[u8]) -> bool {
        let _ = (key, value);
        false
    }

    /// Remove `key` from the ordered index. Returns whether the write was
    /// persisted (see [`index_set`](Self::index_set)).
    fn index_del(&mut self, key: &[u8]) -> bool {
        let _ = key;
        false
    }

    /// Remove every index key beginning with `prefix` (used to clear one
    /// collection's index before a rebuild). Returns whether the write was
    /// persisted (see [`index_set`](Self::index_set)).
    fn index_del_prefix(&mut self, prefix: &[u8]) -> bool {
        let _ = prefix;
        false
    }

    /// Return `(key, value)` pairs in `[lo, hi)`, ascending by key, after
    /// skipping `offset` and capped at `limit` (`None` = unbounded).
    fn index_scan(
        &self,
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let _ = (lo, hi, offset, limit);
        Vec::new()
    }

    /// The largest `(key, value)` in `[lo, hi)` — a reverse seek for
    /// `SortedMap::last` (`O(log n)` instead of a forward walk to the end).
    fn index_last(&self, lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let _ = (lo, hi);
        None
    }
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryStorage {
    inner: BTreeMap<Key, Value>,
    /// Ordered secondary index (see `Storage`'s index methods). A `BTreeMap`
    /// iterates in key order, mirroring the RocksDB `SortedIndex` column the
    /// node backs this with in production.
    index: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Storage for InMemoryStorage {
    fn get(&self, key: &Key) -> Option<Value> {
        debug!(target: "runtime::storage::memory", key_len = key.len(), "InMemoryStorage::get");
        self.inner.get(key).cloned()
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        debug!(
            target: "runtime::storage::memory",
            key_len = key.len(),
            value_len = value.len(),
            "InMemoryStorage::set"
        );
        self.inner.insert(key, value)
    }

    // todo! revisit this, should we return the value by default?
    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        debug!(target: "runtime::storage::memory", key_len = key.len(), "InMemoryStorage::remove");
        self.inner.remove(key)
    }

    fn has(&self, key: &Key) -> bool {
        debug!(target: "runtime::storage::memory", key_len = key.len(), "InMemoryStorage::has");
        self.inner.contains_key(key)
    }

    fn index_set(&mut self, key: &[u8], value: &[u8]) -> bool {
        let _ = self.index.insert(key.to_vec(), value.to_vec());
        true
    }

    fn index_del(&mut self, key: &[u8]) -> bool {
        let _ = self.index.remove(key);
        true
    }

    fn index_del_prefix(&mut self, prefix: &[u8]) -> bool {
        self.index.retain(|k, _| !k.starts_with(prefix));
        true
    }

    fn index_scan(
        &self,
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let ordered = self
            .index
            .range(lo.to_vec()..hi.to_vec())
            .map(|(k, v)| (k.clone(), v.clone()))
            .skip(offset);
        match limit {
            Some(n) => ordered.take(n).collect(),
            None => ordered.collect(),
        }
    }

    fn index_last(&self, lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.index
            .range(lo.to_vec()..hi.to_vec())
            .next_back()
            .map(|(k, v)| (k.clone(), v.clone()))
    }
}

impl IntoIterator for InMemoryStorage {
    type Item = (Key, Value);

    type IntoIter = IntoIter<Key, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
