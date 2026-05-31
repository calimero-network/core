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

    /// Best-effort on-disk byte estimate for `col` over `[start, end)`.
    /// Backed by `Database::approximate_size` — for RocksDB this is sampled
    /// from SST metadata (sub-millisecond, no scan); in-memory / default
    /// backends fall back to summing `key+value` lengths.
    pub fn approximate_size(&self, col: Column, start: &[u8], end: &[u8]) -> EyreResult<u64> {
        self.db
            .approximate_size(col, Slice::from(start), Slice::from(end))
    }

    // === Raw, untyped column access (ordered secondary index, core#2559) ===
    //
    // The typed `Handle`/`Entry` API requires fixed-size `KeyComponents`. The
    // `SortedMap` ordered index needs *variable-length* keys in byte order, so
    // these write/iterate raw bytes directly against a column. Intended for the
    // node-local `Column::SortedIndex`; the keys are unhashed so the backend's
    // byte order is the logical key order, making a range scan a native seek.

    /// Write a raw `key -> value` to `col`.
    pub fn raw_put(&self, col: Column, key: &[u8], value: &[u8]) -> EyreResult<()> {
        self.db.put(col, Slice::from(key), Slice::from(value))
    }

    /// Delete a raw `key` from `col`.
    pub fn raw_delete(&self, col: Column, key: &[u8]) -> EyreResult<()> {
        self.db.delete(col, Slice::from(key))
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
}
