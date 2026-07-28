use core::fmt::Debug;
use std::collections::btree_map::IntoIter;
use std::collections::BTreeMap;
use std::ops::Bound;

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

    /// Write the ordered-index validity marker for `key` (a `collection_id`).
    /// This is node-local bookkeeping that MUST live in the same non-synced
    /// keyspace as the ordered index it guards — never in the synced state — so
    /// a peer never observes a marker for an index it did not build. Returns
    /// whether the write was persisted. Default is inert (see [`index_set`]).
    ///
    /// [`index_set`]: Self::index_set
    fn index_meta_set(&mut self, key: &[u8], value: &[u8]) -> bool {
        let _ = (key, value);
        false
    }

    /// Read the ordered-index validity marker for `key` (a `collection_id`)
    /// written by [`index_meta_set`](Self::index_meta_set). Default is inert.
    fn index_meta_get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let _ = key;
        None
    }

    /// Delete the ordered-index validity marker for `key` (a `collection_id`),
    /// forcing the next ordered read to rebuild the index. The sync/apply path
    /// calls this to invalidate a collection whose element set changed outside
    /// `insert` (see `calimero_storage`'s `index_meta_clear`). Returns whether
    /// the write was persisted. Default is inert (see [`index_set`]).
    ///
    /// [`index_set`]: Self::index_set
    fn index_meta_del(&mut self, key: &[u8]) -> bool {
        let _ = key;
        false
    }
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryStorage {
    inner: BTreeMap<Key, Value>,
    /// Ordered secondary index (see `Storage`'s index methods). A `BTreeMap`
    /// iterates in key order, mirroring the RocksDB `SortedIndex` column the
    /// node backs this with in production.
    index: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Node-local ordered-index validity markers (see `index_meta_*`), kept
    /// separate from `index` so a marker can never be returned by an index
    /// range scan — mirroring the RocksDB `SortedIndexMeta` sibling column.
    index_meta: BTreeMap<Vec<u8>, Vec<u8>>,
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
        // Range directly over the `&[u8]` bounds (`Vec<u8>: Borrow<[u8]>`)
        // rather than allocating an owned `Vec` for each endpoint per scan.
        let ordered = self
            .index
            .range::<[u8], _>((Bound::Included(lo), Bound::Excluded(hi)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .skip(offset);
        match limit {
            Some(n) => ordered.take(n).collect(),
            None => ordered.collect(),
        }
    }

    fn index_last(&self, lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.index
            .range::<[u8], _>((Bound::Included(lo), Bound::Excluded(hi)))
            .next_back()
            .map(|(k, v)| (k.clone(), v.clone()))
    }

    fn index_meta_set(&mut self, key: &[u8], value: &[u8]) -> bool {
        let _ = self.index_meta.insert(key.to_vec(), value.to_vec());
        true
    }

    fn index_meta_get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.index_meta.get(key).cloned()
    }

    fn index_meta_del(&mut self, key: &[u8]) -> bool {
        let _ = self.index_meta.remove(key);
        true
    }
}

impl IntoIterator for InMemoryStorage {
    type Item = (Key, Value);

    type IntoIter = IntoIter<Key, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
