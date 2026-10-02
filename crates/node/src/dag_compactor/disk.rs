//! Pruning one context's rows in the delta column by what the column holds.
//!
//! The in-memory DAG is not a count of a context's rows. A context nothing has
//! touched since the node started has no DAG at all, and after a restart a
//! compacted context's DAG cannot be rebuilt from its rows: the oldest retained
//! row's parent was pruned, so `load_persisted_deltas` restores none of the
//! chain above it, and those rows are reached only lazily, as the parents of
//! new deltas. So both whether a context is eligible and which rows are its
//! recent window are read from the column itself, in bounded passes:
//!
//! 1. count the context's rows, reading at most `min_rows + 1` keys;
//! 2. walk back from the persisted `dag_heads` through each row's parents,
//!    retaining the first `retain_count` or so (the soft floor
//!    `DagStore::prune_to_recent` uses, so the two windows agree);
//! 3. scan at most [`MAX_COMPACTION_SCAN_ROWS`] rows and delete, in batches,
//!    up to [`MAX_COMPACTION_DELETES_PER_CONTEXT`] applied rows that are
//!    neither retained, a head, nor held by the caller.
//!
//! A row that is not `applied` is never deleted: it is a pending delta, or one
//! whose commit a crash interrupted, and `load_persisted_deltas` re-drives it.
//!
//! The caller holds the context's execution lock across all of it. Every path
//! that commits an applied row together with the heads holds that lock (the
//! executor for a local delta, the inbound apply across its heads commit), so
//! no head can move and no applied row can land between the window being
//! drawn and the rows outside it being deleted. The heads are re-read before
//! every delete batch anyway, and a change stops the pass: a writer that does
//! not take the lock (a snapshot's checkpoint rows) cannot then have its rows
//! judged against heads that no longer stand.

use std::collections::{HashSet, VecDeque};

use calimero_primitives::context::ContextId;
use calimero_store::db::Column;
use calimero_store::key::{self, AsKeyParts};
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// The genesis parent: never a stored row, never retained or counted.
const GENESIS: [u8; 32] = [0; 32];

/// Most of one context's delta rows a single sweep reads looking for rows to
/// prune. A context holding more is pruned over several sweeps: each reads
/// this many rows from the start of the context's key range, which deletes
/// keep moving on to rows not yet seen.
pub(crate) const MAX_COMPACTION_SCAN_ROWS: usize = 200_000;

/// Most rows a single sweep deletes from one context, so one context with a
/// large backlog cannot turn a sweep into an unbounded burst of deletes.
pub(crate) const MAX_COMPACTION_DELETES_PER_CONTEXT: usize = 100_000;

/// Rows deleted per write batch: each batch is one atomic transaction.
const COMPACTION_DELETE_BATCH: usize = 10_000;

/// What pruning one context's rows did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskPrune {
    /// Rows deleted.
    pub(crate) pruned: usize,
    /// Key and value bytes of the deleted rows.
    pub(crate) bytes: u64,
}

/// Whether `context_id` holds more than `min_rows` rows in the delta column.
/// Reads at most `min_rows + 1` keys.
pub(crate) fn holds_more_rows_than(
    store: &Store,
    context_id: ContextId,
    min_rows: usize,
) -> EyreResult<bool> {
    let handle = store.handle();
    let mut iter = handle.iter::<key::ContextDagDelta>()?;
    let mut next = iter.seek(key::ContextDagDelta::new(context_id, GENESIS))?;
    let mut rows = 0usize;
    while let Some(key) = next {
        if key.context_id() != context_id {
            break;
        }
        rows += 1;
        if rows > min_rows {
            return Ok(true);
        }
        next = iter.next()?;
    }
    Ok(false)
}

/// Prune `context_id`'s rows to the recent window, when it holds more than
/// `min_rows` of them. Rows whose id is in `keep` stay (a live context passes
/// what its in-memory DAG holds, so the disk never drops a delta the DAG still
/// serves or a pending delta still waits on).
///
/// The caller must hold the context's execution lock; see the module docs.
pub(crate) fn prune_context_rows(
    store: &Store,
    context_id: ContextId,
    min_rows: usize,
    retain_count: usize,
    keep: &HashSet<[u8; 32]>,
) -> EyreResult<DiskPrune> {
    // A window this wide is never drawn: the walk would be a scan the bound
    // does not cover, and drawing a narrower one would prune more than asked.
    if retain_count >= MAX_COMPACTION_SCAN_ROWS
        || !holds_more_rows_than(store, context_id, min_rows)?
    {
        return Ok(DiskPrune::default());
    }
    // No heads, no notion of recent: leave the rows alone rather than call
    // every one of them old.
    let heads = persisted_heads(store, context_id)?;
    if heads.iter().all(|head| *head == GENESIS) {
        return Ok(DiskPrune::default());
    }

    let retained = retain_window(store, context_id, &heads, retain_count)?;
    let candidates = prunable_rows(store, context_id, |id| {
        retained.contains(id) || keep.contains(id)
    })?;

    let mut done = DiskPrune::default();
    for batch in candidates.chunks(COMPACTION_DELETE_BATCH) {
        if persisted_heads(store, context_id)? != heads {
            break;
        }
        let mut tx = Transaction::default();
        for (row, _) in batch {
            tx.delete(row);
        }
        store.apply(&tx)?;
        done.pruned += batch.len();
        done.bytes += batch.iter().map(|(_, bytes)| bytes).sum::<u64>();
    }
    Ok(done)
}

/// Compact `context_id`'s slice of the delta column after a prune reclaimed
/// `reclaimed` bytes from it, when that is worth the rewrite (the tombstone
/// GC's bar, [`crate::gc::worth_compacting`]). Returns whether it compacted.
///
/// A deleted row frees nothing by itself: RocksDB writes a deletion marker and
/// the row stays in its SST until a compaction merges the two. Blocking, and
/// run with no lock held, since compaction rewrites files rather than rows.
pub(crate) fn compact_pruned_slice(
    store: &Store,
    context_id: ContextId,
    reclaimed: u64,
) -> EyreResult<bool> {
    let lo = key::ContextDagDelta::new(context_id, GENESIS);
    let hi = key::ContextDagDelta::new(context_id, [u8::MAX; 32]);
    let (lo, hi) = (lo.as_key().as_bytes(), hi.as_key().as_bytes());
    let size = store.approximate_size(Column::Delta, lo, hi)?;
    if !crate::gc::worth_compacting(reclaimed, size) {
        return Ok(false);
    }
    store.raw_compact_range(Column::Delta, lo, hi)?;
    Ok(true)
}

/// The context's persisted DAG heads; empty for an unknown context.
fn persisted_heads(store: &Store, context_id: ContextId) -> EyreResult<Vec<[u8; 32]>> {
    Ok(store
        .handle()
        .get(&key::ContextMeta::new(context_id))?
        .map(|meta| meta.dag_heads)
        .unwrap_or_default())
}

/// The heads plus the first `retain_count` or so rows reachable from them,
/// breadth first. As in `DagStore::prune_to_recent`, the budget is checked
/// once per expanded row, so the set can exceed it by one row's parents; heads
/// are retained whatever the budget. Reads at most one row per retained id.
fn retain_window(
    store: &Store,
    context_id: ContextId,
    heads: &[[u8; 32]],
    retain_count: usize,
) -> EyreResult<HashSet<[u8; 32]>> {
    let handle = store.handle();
    let mut retained: HashSet<[u8; 32]> = heads
        .iter()
        .copied()
        .filter(|head| *head != GENESIS)
        .collect();
    let mut queue: VecDeque<[u8; 32]> = retained.iter().copied().collect();
    while retained.len() < retain_count {
        let Some(id) = queue.pop_front() else { break };
        let Some(row) = handle.get(&key::ContextDagDelta::new(context_id, id))? else {
            continue;
        };
        for parent in row.parents {
            if parent != GENESIS && retained.insert(parent) {
                queue.push_back(parent);
            }
        }
    }
    Ok(retained)
}

/// Applied rows of the context that `kept` does not claim, with their key and
/// value bytes. Reads at most [`MAX_COMPACTION_SCAN_ROWS`] keys and returns at
/// most [`MAX_COMPACTION_DELETES_PER_CONTEXT`] rows.
fn prunable_rows(
    store: &Store,
    context_id: ContextId,
    kept: impl Fn(&[u8; 32]) -> bool,
) -> EyreResult<Vec<(key::ContextDagDelta, u64)>> {
    let handle = store.handle();
    let mut iter = handle.iter::<key::ContextDagDelta>()?;
    let mut next = iter.seek(key::ContextDagDelta::new(context_id, GENESIS))?;
    let mut scanned = 0usize;
    let mut rows = Vec::new();
    while let Some(row_key) = next {
        if row_key.context_id() != context_id
            || scanned >= MAX_COMPACTION_SCAN_ROWS
            || rows.len() >= MAX_COMPACTION_DELETES_PER_CONTEXT
        {
            break;
        }
        scanned += 1;
        if !kept(&row_key.delta_id()) {
            if let Some(row) = handle.get(&row_key)? {
                if row.applied {
                    let bytes = row_key.as_key().as_bytes().len() + borsh::object_length(&row)?;
                    rows.push((row_key, bytes as u64));
                }
            }
        }
        next = iter.next()?;
    }
    Ok(rows)
}
