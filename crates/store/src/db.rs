use core::fmt::Debug;

use eyre::Result as EyreResult;
use strum::{AsRefStr, EnumIter};

use crate::config::StoreConfig;
use crate::iter::Iter;
use crate::slice::Slice;
use crate::tx::Transaction;

mod memory;

pub use memory::InMemoryDB;

#[derive(AsRefStr, Clone, Copy, Debug, EnumIter, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Column {
    Meta,
    Config,
    Identity,
    State,
    /// Node-local private storage that is NOT synchronized across nodes
    PrivateState,
    Delta,
    Blobs,
    Application,
    Alias,
    Generic,
    Group,
    /// Node-local context-scoped state. NOT synchronized across nodes.
    /// Used for things like the per-(member, context) `leave_context`
    /// tombstone — sync-and-auto-follow stop on the node where the user
    /// opted out, while peers see no change.
    ContextLocal,
    /// Node-local secondary index for `SortedMap` collections. NOT
    /// synchronized across nodes — it is a derived materialised view of the
    /// synced entity set, maintained locally as entries are written/applied.
    /// Keys are `collection_id(32) ‖ order_key_bytes` (unhashed, so RocksDB's
    /// byte order = key order), values are the entry's 32-byte id. Enables
    /// `O(log n + k)` range/prefix/pagination over `SortedMap` (core#2559).
    SortedIndex,
}

pub trait Database<'a>: Debug + Send + Sync + 'static {
    fn open(config: &StoreConfig) -> EyreResult<Self>
    where
        Self: Sized;

    fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool>;
    fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>>;
    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> EyreResult<()>;
    fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()>;

    // TODO: We should consider returning Iterator here.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "TODO: This should be implemented"
    )]
    fn iter(&self, col: Column) -> EyreResult<Iter<'_>>;

    // todo! redesign this, each DB should return a transaction
    // todo! modelled similar to Iter - {put, delete, clear}
    fn apply(&self, tx: &Transaction<'a>) -> EyreResult<()>;

    /// Returns an iterator over a column with a consistent snapshot view.
    ///
    /// The iterator sees a frozen point-in-time view of the database,
    /// unaffected by concurrent writes. This is essential for operations
    /// that need to iterate over a consistent state (e.g., snapshot generation).
    ///
    /// The default implementation falls back to `iter()` for databases that
    /// don't support snapshots natively.
    fn iter_snapshot(&self, col: Column) -> EyreResult<Iter<'_>> {
        self.iter(col)
    }

    /// The largest `(key, value)` in `col` over `[lo, hi)` — i.e. a reverse
    /// seek to the last key in the range. Used for `SortedMap::last` (an
    /// `O(log n)` "max key" lookup instead of a forward walk to the end).
    ///
    /// The default does a forward `O(n)` scan keeping the last in-range entry,
    /// so backends without reverse iteration still work; RocksDB overrides this
    /// with `seek_for_prev` for a true `O(log n)` seek.
    fn last_in_range(
        &self,
        col: Column,
        lo: Slice<'_>,
        hi: Slice<'_>,
    ) -> EyreResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut iter = self.iter(col)?;
        let mut last: Option<(Vec<u8>, Vec<u8>)> = None;
        let mut pos = iter.seek(lo)?.map(|k| k.as_ref().to_vec());
        while let Some(key) = pos {
            if key.as_slice() >= hi.as_ref() {
                break;
            }
            let value = iter.read()?.as_ref().to_vec();
            last = Some((key, value));
            pos = iter.next()?.map(|k| k.as_ref().to_vec());
        }
        Ok(last)
    }

    /// Delete every key in `col` over `[lo, hi)` in one shot. Used to drop a
    /// `SortedMap`'s whole index slice on `clear()` without materialising the
    /// key set in memory first.
    ///
    /// The default buffers the in-range keys and deletes them one by one (so
    /// backends without a native range delete still work); RocksDB overrides
    /// this with `delete_range_cf`, a single range tombstone — `O(1)` write,
    /// no per-key I/O and no unbounded buffer.
    fn delete_range(&self, col: Column, lo: Slice<'_>, hi: Slice<'_>) -> EyreResult<()> {
        let mut iter = self.iter(col)?;
        let hi_bytes: Vec<u8> = hi.as_ref().to_vec();
        let mut keys: Vec<Vec<u8>> = Vec::new();
        let mut pos = iter.seek(lo)?.map(|k| k.as_ref().to_vec());
        while let Some(key) = pos {
            if key.as_slice() >= hi_bytes.as_slice() {
                break;
            }
            keys.push(key);
            pos = iter.next()?.map(|k| k.as_ref().to_vec());
        }
        drop(iter);
        for key in keys {
            self.delete(col, Slice::from(&key))?;
        }
        Ok(())
    }

    /// Best-effort estimate of on-disk bytes stored in `col` for keys in
    /// `[start, end)`. Used by usage-reporting endpoints to measure
    /// per-group / per-context storage cheaply (RocksDB returns a real
    /// approximation from SST metadata, no scan). Callers must not
    /// depend on exact byte counts.
    ///
    /// Default implementation walks the column (seeking to `start`) and
    /// sums actual `key.len() + value.len()` for matching entries.
    /// Backends backed by sorted storage should override with something
    /// cheaper (RocksDB: `get_approximate_sizes_cf`).
    fn approximate_size(&self, col: Column, start: Slice<'_>, end: Slice<'_>) -> EyreResult<u64> {
        let mut iter = self.iter(col)?;
        let mut total: u64 = 0;
        let end_bytes: Vec<u8> = end.as_ref().to_vec();

        // Seek to `start` and drive the iteration to sum matching entries.
        // We copy each key's bytes out (to release the mutable borrow),
        // `read()` the value (immutable borrow), record the size, then
        // drop both and advance.
        let mut current: Option<Vec<u8>> = iter.seek(start)?.map(|k| k.as_ref().to_vec());
        while let Some(key_bytes) = current {
            if key_bytes.as_slice() >= end_bytes.as_slice() {
                break;
            }
            let value_len = {
                let value = iter.read()?;
                value.as_ref().len() as u64
            };
            total = total.saturating_add(key_bytes.len() as u64);
            total = total.saturating_add(value_len);
            current = iter.next()?.map(|k| k.as_ref().to_vec());
        }
        Ok(total)
    }
}
