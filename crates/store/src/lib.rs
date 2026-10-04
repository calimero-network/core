use std::sync::Arc;

use eyre::Result as EyreResult;

#[cfg(feature = "datatypes")]
pub mod batch;
pub mod config;
pub mod db;
pub mod entry;
pub mod handle;
pub mod iter;
pub mod key;
pub mod layer;
#[cfg(feature = "datatypes")]
pub mod namespace_signer;
pub mod slice;
pub mod tx;

use config::StoreConfig;
use db::{Column, Database};
pub use handle::Handle;
use slice::Slice;

#[cfg(feature = "datatypes")]
pub mod types;

#[cfg(feature = "datatypes")]
pub use batch::StoreBatch;

#[derive(Clone, Debug)]
pub struct Store {
    db: Arc<dyn for<'a> Database<'a>>,
}

impl Store {
    /// Creates a new `Store` from an existing database instance. Useful for testing.
    pub fn new(db: Arc<dyn for<'a> Database<'a>>) -> Self {
        Self { db }
    }

    pub fn open<T: for<'a> Database<'a>>(config: &StoreConfig) -> EyreResult<Self> {
        let db = T::open(config)?;
        Ok(Self { db: Arc::new(db) })
    }

    #[must_use]
    pub fn handle(&self) -> Handle<Self> {
        Handle::new(self.clone())
    }

    /// Flush in-memory buffers (memtable + WAL) to durable storage.
    ///
    /// Invoked on a controlled shutdown so an abrupt process stop immediately
    /// afterwards cannot lose recently-written state. No-op for non-persistent
    /// backends. This is a best-effort durability barrier and does not close
    /// the database — the `Arc<dyn Database>` may still be shared elsewhere.
    pub fn flush(&self) -> EyreResult<()> {
        self.db.flush()
    }

    /// Cheap reachability probe used by the readiness/health endpoints.
    ///
    /// Performs a single point lookup against a metadata column: it exercises
    /// the column-family handle and read path without materialising an
    /// iterator, and surfaces an `Err` if the underlying database has become
    /// unavailable. A missing key is success — we only care that the read
    /// completed.
    pub fn ping(&self) -> EyreResult<()> {
        let _ = self.db.get(Column::Meta, Slice::from(&[][..]))?;
        Ok(())
    }

    /// Best-effort on-disk byte estimate for `col` over `[start, end)`.
    /// Backed by `Database::approximate_size` — for RocksDB this is sampled
    /// from SST metadata (sub-millisecond, no scan); in-memory / default
    /// backends fall back to summing `key+value` lengths.
    pub fn approximate_size(&self, col: Column, start: &[u8], end: &[u8]) -> EyreResult<u64> {
        self.db
            .approximate_size(col, Slice::from(start), Slice::from(end))
    }

    /// Table-file accounting for the whole store (see
    /// [`Database::table_stats`]). `None` for backends without table files.
    pub fn table_stats(&self) -> EyreResult<Option<db::TableStats>> {
        self.db.table_stats()
    }

    // === Raw, untyped column access (ordered secondary index, core#2559) ===
    //
    // The typed `Handle`/`Entry` API requires fixed-size `KeyComponents`. The
    // `SortedMap` ordered index needs *variable-length* keys in byte order, so
    // these write/iterate raw bytes directly against a column. Intended for the
    // node-local `Column::SortedIndex`; the keys are unhashed so the backend's
    // byte order is the logical key order, making a range scan a native seek.

    /// Compact `col` over `[lo, hi)` now (see [`Database::compact_range`]).
    /// Blocking.
    ///
    /// # Errors
    ///
    /// The backend's failure, or an unknown column.
    pub fn raw_compact_range(&self, col: Column, lo: &[u8], hi: &[u8]) -> EyreResult<()> {
        self.db.compact_range(col, Slice::from(lo), Slice::from(hi))
    }

    /// Write a raw `key -> value` to `col`.
    pub fn raw_put(&self, col: Column, key: &[u8], value: &[u8]) -> EyreResult<()> {
        self.db.put(col, Slice::from(key), Slice::from(value))
    }

    /// Point-read a raw `key` from `col`, returning the owned value bytes if
    /// present. Used for node-local single-key lookups (e.g. the `SortedMap`
    /// ordered-index validity marker in `Column::SortedIndexMeta`) where the
    /// variable-length key can't go through the typed `Handle`/`Entry` API.
    pub fn raw_get(&self, col: Column, key: &[u8]) -> EyreResult<Option<Vec<u8>>> {
        Ok(self
            .db
            .get(col, Slice::from(key))?
            .map(|slice| slice.into_boxed().into_vec()))
    }

    /// Delete a raw `key` from `col`.
    pub fn raw_delete(&self, col: Column, key: &[u8]) -> EyreResult<()> {
        self.db.delete(col, Slice::from(key))
    }

    /// Delete every raw key in `col` over `[lo, hi)` in one shot (a native
    /// range tombstone on RocksDB). Used to drop a whole index slice on
    /// `clear()` without buffering the key set.
    pub fn raw_delete_range(&self, col: Column, lo: &[u8], hi: &[u8]) -> EyreResult<()> {
        self.db.delete_range(col, Slice::from(lo), Slice::from(hi))
    }

    /// Delete every raw key in `col` that starts with `prefix`.
    ///
    /// One native range tombstone over `[prefix, successor(prefix))` when the
    /// prefix has a successor. A prefix of only `0xFF` bytes has none — its keys
    /// run to the end of the column — so those are walked and deleted in
    /// batches instead.
    pub fn raw_delete_prefix(&self, col: Column, prefix: &[u8]) -> EyreResult<()> {
        if let Some(hi) = prefix_successor(prefix) {
            return self.raw_delete_range(col, prefix, &hi);
        }

        /// Keys collected per pass before deleting, bounding peak memory.
        const BATCH: usize = 4096;

        loop {
            let mut keys = Vec::new();
            {
                let mut iter = self.db.iter(col)?;
                let mut pos = iter.seek(Slice::from(prefix))?.map(|k| k.as_ref().to_vec());
                while let Some(key) = pos {
                    if !key.starts_with(prefix) || keys.len() == BATCH {
                        break;
                    }
                    keys.push(key);
                    pos = iter.next()?.map(|k| k.as_ref().to_vec());
                }
            }

            let done = keys.len() < BATCH;
            for key in keys {
                self.db.delete(col, Slice::from(key.as_slice()))?;
            }
            if done {
                return Ok(());
            }
        }
    }

    /// Collect up to `max` `(key, value)` pairs in `col` over `[lo, hi)`,
    /// ascending by key (the backend's native order). One forward seek + walk,
    /// stopping after `max` items (`None` = unbounded) so a bounded query
    /// (`page`/`range` with a limit) walks `O(max)` rather than the whole range.
    pub fn raw_scan(
        &self,
        col: Column,
        lo: &[u8],
        hi: &[u8],
        max: Option<usize>,
    ) -> EyreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if max == Some(0) {
            return Ok(Vec::new());
        }
        let mut iter = self.db.iter(col)?;
        let mut out = Vec::new();
        // Copy the key bytes out of each borrow before reading the value /
        // advancing, so we never hold the iterator borrow across calls.
        let mut pos = iter.seek(Slice::from(lo))?.map(|k| k.as_ref().to_vec());
        while let Some(key) = pos {
            if key.as_slice() >= hi {
                break;
            }
            let value = iter.read()?.as_ref().to_vec();
            out.push((key, value));
            if max.is_some_and(|m| out.len() >= m) {
                break;
            }
            pos = iter.next()?.map(|k| k.as_ref().to_vec());
        }
        Ok(out)
    }

    /// The largest `(key, value)` in `col` over `[lo, hi)` (a reverse seek —
    /// `O(log n)` on RocksDB). Used for `SortedMap::last`.
    pub fn raw_last(
        &self,
        col: Column,
        lo: &[u8],
        hi: &[u8],
    ) -> EyreResult<Option<(Vec<u8>, Vec<u8>)>> {
        self.db.last_in_range(col, Slice::from(lo), Slice::from(hi))
    }

    /// Atomically commit a multi-key [`Transaction`](tx::Transaction) as a
    /// single backend write. On RocksDB this is one `WriteBatch`, so either
    /// every put/delete in `tx` lands or none do — there is no state where
    /// part of the batch is persisted. This is the primitive callers reach
    /// for when several keys (possibly across columns) must move together,
    /// e.g. cascade-delta records plus their context's `dag_heads`.
    pub fn apply(&self, tx: &tx::Transaction<'_>) -> EyreResult<()> {
        self.db.apply(tx)
    }
}

/// The smallest byte string greater than every string that starts with
/// `prefix`: the prefix with its last non-`0xFF` byte incremented and anything
/// after it dropped. `None` when every byte is `0xFF` (or `prefix` is empty),
/// since then no finite upper bound exists.
fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let last = prefix.iter().rposition(|byte| *byte != 0xFF)?;
    let mut hi = prefix[..=last].to_vec();
    hi[last] = hi[last].wrapping_add(1);
    Some(hi)
}

#[cfg(test)]
mod raw_delete_prefix_tests {
    use std::sync::Arc;

    use super::db::{Column, InMemoryDB};
    use super::{prefix_successor, Store};

    #[test]
    fn successor_increments_the_last_byte_that_can_grow() {
        assert_eq!(prefix_successor(&[0x01, 0x02]), Some(vec![0x01, 0x03]));
        assert_eq!(prefix_successor(&[0x01, 0xFF, 0xFF]), Some(vec![0x02]));
        assert_eq!(prefix_successor(&[0xFF, 0xFF]), None);
        assert_eq!(prefix_successor(&[]), None);
    }

    #[test]
    fn deletes_exactly_the_prefixed_keys_including_an_all_ff_prefix() {
        // An all-0xFF prefix has no successor, so it takes the walking path;
        // both paths must delete the same set.
        for prefix in [[0x42_u8; 4], [0xFF_u8; 4]] {
            let store = Store::new(Arc::new(InMemoryDB::owned()));
            let mut before = prefix.to_vec();
            before[3] = before[3].wrapping_sub(1);

            let inside = [
                prefix.to_vec(),
                [prefix.as_slice(), &[0x00]].concat(),
                [prefix.as_slice(), &[0xFF; 9]].concat(),
            ];
            for key in inside.iter().chain([&before]) {
                store.raw_put(Column::State, key, b"v").expect("put");
            }

            store
                .raw_delete_prefix(Column::State, &prefix)
                .expect("delete should succeed");

            for key in &inside {
                assert!(
                    store.raw_get(Column::State, key).expect("get").is_none(),
                    "{key:x?} starts with {prefix:x?} and must be deleted"
                );
            }
            assert!(
                store
                    .raw_get(Column::State, &before)
                    .expect("get")
                    .is_some(),
                "{before:x?} does not start with {prefix:x?} and must survive"
            );
        }
    }
}
