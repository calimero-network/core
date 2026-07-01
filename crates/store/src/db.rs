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
    /// Node-local durable buffer for absorbed straggler deltas (PR-6b). Holds
    /// the original signed bytes of a state delta that arrived under a schema
    /// the locally-loaded binary cannot yet read, so it is replayed verbatim
    /// once the binary advances rather than silently dropped. NOT synchronized
    /// across nodes. Auto-created from `Column::iter()` at `open_cf` (no DB
    /// migration). Keys are `prefix(1) ‖ context(32) ‖ producing_app_key(32) ‖
    /// delta_id(32)`; values are borsh'd `AbsorbRecord`s.
    AbsorbBuffer,
    /// Node-local per-context marker that the last migration attempt did not
    /// complete (read by the migration heartbeat). Its own column so its
    /// `context_id`-only key cannot collide with the same-shaped key in
    /// `ContextLocal` (e.g. `ContextAuthoredRemaining`). NOT synchronized;
    /// auto-created from `Column::iter()` at `open_cf` (no DB migration).
    ContextMigrationFailed,
    /// Node-local per-context pin to the bytecode blob the context's
    /// committed state executes under, written by a logically-aborted
    /// migration: the version-stable bundle ApplicationId's row already holds
    /// the NEW bytecode, so the pin keeps reads on the old code until a
    /// migrate succeeds. Own column for the same collision reason as above.
    /// NOT synchronized; auto-created from `Column::iter()` (no DB migration).
    ContextExecutingBlob,
    ContextActivatedBlob,
    /// Node-local per-application breadcrumb of the bytecode blob an in-place
    /// (same-id) bundle install overwrote — the source for the executing-blob
    /// pin above and the L1 downgrade gate's pre-install ABI. Own column: the
    /// `application_id`-only key shape would collide with `ApplicationMeta`
    /// in `Application`. NOT synchronized; auto-created from `Column::iter()`
    /// (no DB migration).
    ApplicationPreviousBlob,
    /// Node-local per-context marker that an operator requested a full-state
    /// resync (the recovery for a stranded `NoMigrationPath` context). Presence
    /// is the signal — it admits a snapshot full-state replacement and is
    /// cleared once the snapshot completes. Own column for the same
    /// `context_id`-only collision reason as the markers above. NOT
    /// synchronized; auto-created from `Column::iter()` (no DB migration).
    ContextResyncRequested,
    /// The unified causal-log op-store (cutover C2): one borsh'd `calimero_op::Op`
    /// per row, keyed `scope(32) ‖ op_id(32)` (see `ScopeUnifiedOp`) — the prefix is
    /// the op's scope, so a scope's op-log is a contiguous range. Its own column
    /// during the dual-write transition so its `(scope, op_id)` key cannot collide
    /// with the same-shaped legacy delta key in `Delta`; the two merge once the
    /// legacy delta rows retire (C2.5). Synchronized like the planes it will
    /// subsume. Auto-created from `Column::iter()` at `open_cf` (no DB migration).
    UnifiedOp,
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
    /// The default deletes in-range keys in bounded batches (so backends
    /// without a native range delete still work); RocksDB overrides this with
    /// `delete_range_cf`, a single range tombstone — `O(1)` write, no per-key
    /// I/O and no buffer.
    ///
    /// Each batch collects at most `DELETE_RANGE_BATCH` keys, deletes them, then
    /// re-seeks to the last key of that batch to resume: it was just deleted, so
    /// the seek lands on the next surviving in-range key. Resuming from the batch
    /// boundary (rather than re-seeking to `lo` every time) keeps each seek local
    /// to where the previous one stopped. Peak memory is one batch regardless of
    /// range size, where buffering the whole range first would grow without
    /// bound on a large slice.
    ///
    /// Not atomic: this issues per-key deletes, so an error mid-range leaves the
    /// keys deleted so far gone and the rest intact. Callers that need
    /// all-or-nothing must wrap it in their own transaction. (The prior
    /// buffer-everything default had the same non-atomicity.)
    fn delete_range(&self, col: Column, lo: Slice<'_>, hi: Slice<'_>) -> EyreResult<()> {
        /// Keys buffered (and deleted) per batch. Bounds peak memory; a larger
        /// value trades memory for fewer re-seeks.
        const DELETE_RANGE_BATCH: usize = 4096;

        let hi_bytes: Vec<u8> = hi.as_ref().to_vec();
        // First batch seeks to `lo`; later batches resume from the last-deleted
        // key (which is gone, so the seek advances to the next survivor).
        let mut resume_from: Vec<u8> = lo.as_ref().to_vec();

        loop {
            let mut iter = self.iter(col)?;
            let mut keys: Vec<Vec<u8>> = Vec::new();
            let mut pos = iter
                .seek(Slice::from(&resume_from))?
                .map(|k| k.as_ref().to_vec());
            while let Some(key) = pos {
                if key.as_slice() >= hi_bytes.as_slice() {
                    break;
                }
                keys.push(key);
                if keys.len() >= DELETE_RANGE_BATCH {
                    break;
                }
                pos = iter.next()?.map(|k| k.as_ref().to_vec());
            }
            drop(iter);

            if keys.is_empty() {
                break;
            }

            let drained = keys.len();
            // The last key of the batch is where the next seek resumes.
            if let Some(last) = keys.last() {
                resume_from.clone_from(last);
            }
            for key in keys {
                self.delete(col, Slice::from(&key))?;
            }

            // A short batch means we reached `hi`; no more keys remain.
            if drained < DELETE_RANGE_BATCH {
                break;
            }
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
