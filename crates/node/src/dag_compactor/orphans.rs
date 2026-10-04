//! Sweeping side rows whose delta is gone.
//!
//! Until a delta's side rows went with its row ([`super::disk`]), every
//! compaction left the side rows of the deltas it pruned behind, and nothing
//! else removes one: they stay on disk for good. This sweep finds them.
//!
//! A side row's key is hashed from `(context, delta id)`, so it names neither:
//! it cannot be looked up from its own key, and a context's rows are not a
//! range. A side row is told live by computing the keys of everything that
//! may still read one, and an orphan is a side row none of them claims:
//!
//! - every row in the delta column, of every context, including one deleted
//!   (its rows stay servable) or not yet in `ContextMeta`;
//! - every record in the absorb buffer (an absorbed delta's replay reads its
//!   trigger back, with no delta row yet);
//! - every delta a live in-memory DAG holds, applied or pending, and every
//!   member delta waiting on its anchor (a pending delta has no row, and its
//!   side rows are served once it applies).
//!
//! Nothing else reads a side row (see [`super::side_rows`]), so an orphan is
//! read by nothing, and deleting it changes no answer this node gives: the
//! responder looks a delta's side rows up only after finding its row, and a
//! delta with no row is `DeltaNotFound` either way.
//!
//! No context lock is held: an orphan's context cannot be known from its key.
//! What the lock would exclude is a side row recorded just ahead of its delta
//! (every receive path records the events hash before the delta enters the
//! DAG, and the trigger before the HLC fence), which in that instant has
//! nothing that claims it. So a side row is deleted only after two sweeps,
//! an interval apart, both found it unclaimed: one in flight on the first
//! look is in the DAG, on disk or absorbed by the second. One that is still
//! unclaimed then belongs to a delta the node dropped (refused, fenced, or a
//! pending delta that aged out or was lost to a restart); every receive path
//! records it again if that delta comes back.
//!
//! Each sweep reads at most [`MAX_ORPHAN_SCAN_ROWS`] rows of each table, from
//! where the last sweep left off, and the claims of at most
//! [`MAX_ORPHAN_LIVE_ROWS`] deltas; a node holding more decides nothing. The
//! second look re-reads the same window, and only then does the table move on
//! (wrapping round at its end).

use std::collections::{HashMap, HashSet};

use calimero_primitives::context::ContextId;
use calimero_store::db::Column;
use calimero_store::key::{self, AsKeyParts, FRAGMENT_SIZE, SCOPE_SIZE};
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::debug;

use super::disk::COMPACTION_DELETE_BATCH;
use super::side_rows::SideTable;

/// Most rows of one side table a sweep reads looking for orphans.
pub(crate) const MAX_ORPHAN_SCAN_ROWS: usize = 50_000;

/// Most deltas (delta rows, absorb records and in-memory deltas together) a
/// sweep reads the claims of. Reading them is a walk of the delta column and
/// a hash per side table each, a few seconds at this bound; a node holding
/// more sweeps no orphans.
pub(crate) const MAX_ORPHAN_LIVE_ROWS: usize = 2_000_000;

type Fragment = [u8; FRAGMENT_SIZE];

/// Where each side table's sweep stands, carried from one sweep to the next.
#[derive(Debug, Default)]
pub(crate) struct OrphanSweep {
    tables: [Cursor; SideTable::ALL.len()],
}

#[derive(Debug, Default)]
struct Cursor {
    /// The first row of the window being looked at; the table's start if none.
    from: Option<Fragment>,
    /// Rows the first look at the window found unclaimed.
    suspects: HashSet<Fragment>,
}

/// One sweep's read of each table, taken before the claims are.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    tables: [Window; SideTable::ALL.len()],
}

#[derive(Debug, Default)]
struct Window {
    /// The window's rows still unclaimed, with their key and value bytes.
    rows: HashMap<Fragment, u64>,
    /// The first row past the window; `None` when the window reached the end.
    next: Option<Fragment>,
}

/// What a sweep deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Swept {
    /// Side rows deleted.
    pub(crate) rows: usize,
    /// Their key and value bytes.
    pub(crate) bytes: u64,
}

impl OrphanSweep {
    /// Read each table's window: the side rows to judge this sweep. Read
    /// before the claims, so a side row is judged against claims at least as
    /// new as itself.
    pub(crate) fn read_windows(&self, store: &Store) -> EyreResult<Windows> {
        let mut windows = Windows::default();
        for ((table, cursor), window) in SideTable::ALL
            .into_iter()
            .zip(&self.tables)
            .zip(&mut windows.tables)
        {
            let from = key::Generic::new(table.scope(), cursor.from.unwrap_or([0; FRAGMENT_SIZE]));
            // Past the table's last key: every key here is the same length.
            let (_, last) = table.range();
            let mut end = last.as_key().as_bytes().to_vec();
            end.push(0);
            // One row more than the window, to know where the next one starts.
            let rows = store.raw_scan(
                Column::Generic,
                from.as_key().as_bytes(),
                &end,
                Some(MAX_ORPHAN_SCAN_ROWS + 1),
            )?;
            for (row, value) in rows {
                let Some(fragment) = row
                    .get(SCOPE_SIZE..)
                    .and_then(|f| Fragment::try_from(f).ok())
                else {
                    continue;
                };
                if window.rows.len() >= MAX_ORPHAN_SCAN_ROWS {
                    window.next = Some(fragment);
                    break;
                }
                let _previous = window
                    .rows
                    .insert(fragment, (row.len() + value.len()) as u64);
            }
        }
        Ok(windows)
    }

    /// Drop from `windows` every side row something still claims (`held`: the
    /// deltas live in-memory DAGs hold), delete the rows unclaimed on both
    /// looks at their window, and move each table on once its window had its
    /// second look (or the first found nothing to look at again).
    pub(crate) fn settle(
        &mut self,
        store: &Store,
        mut windows: Windows,
        held: &[(ContextId, [u8; 32])],
    ) -> EyreResult<Swept> {
        if windows.tables.iter().all(|window| window.rows.is_empty()) {
            for (cursor, window) in self.tables.iter_mut().zip(&windows.tables) {
                cursor.suspects.clear();
                cursor.from = window.next;
            }
            return Ok(Swept::default());
        }
        if !rule_out_claimed(store, &mut windows, held)? {
            debug!(
                bound = MAX_ORPHAN_LIVE_ROWS,
                "Orphaned side rows not swept: more deltas than the sweep reads"
            );
            for cursor in &mut self.tables {
                cursor.suspects.clear();
            }
            return Ok(Swept::default());
        }

        let mut swept = Swept::default();
        for ((table, cursor), window) in SideTable::ALL
            .into_iter()
            .zip(&mut self.tables)
            .zip(windows.tables)
        {
            if cursor.suspects.is_empty() {
                if window.rows.is_empty() {
                    cursor.from = window.next;
                } else {
                    cursor.suspects = window.rows.into_keys().collect();
                }
                continue;
            }
            let orphans: Vec<(key::Generic, u64)> = window
                .rows
                .into_iter()
                .filter(|(fragment, _)| cursor.suspects.contains(fragment))
                .map(|(fragment, bytes)| (key::Generic::new(table.scope(), fragment), bytes))
                .collect();
            for batch in orphans.chunks(COMPACTION_DELETE_BATCH) {
                let mut tx = Transaction::default();
                for (row, _) in batch {
                    tx.delete(row);
                }
                store.apply(&tx)?;
                swept.rows += batch.len();
                swept.bytes += batch.iter().map(|(_, bytes)| bytes).sum::<u64>();
            }
            cursor.suspects.clear();
            cursor.from = window.next;
        }
        Ok(swept)
    }
}

/// Remove from `windows` the side rows of every delta that still has a
/// reader: the delta column's rows, the absorb buffer's records and `held`.
/// `false` when there are more than [`MAX_ORPHAN_LIVE_ROWS`] of them, and
/// nothing can be told an orphan.
fn rule_out_claimed(
    store: &Store,
    windows: &mut Windows,
    held: &[(ContextId, [u8; 32])],
) -> EyreResult<bool> {
    let mut claims = 0usize;
    let mut claim = |context_id: ContextId, delta_id: [u8; 32]| {
        claims += 1;
        for (table, window) in SideTable::ALL.into_iter().zip(&mut windows.tables) {
            let _claimed = window
                .rows
                .remove(&table.key(&context_id, &delta_id).fragment());
        }
        claims <= MAX_ORPHAN_LIVE_ROWS
    };

    for (context_id, delta_id) in held {
        if !claim(*context_id, *delta_id) {
            return Ok(false);
        }
    }

    let handle = store.handle();
    let mut deltas = handle.iter::<key::ContextDagDelta>()?;
    for row in deltas.keys() {
        let row = row?;
        if !claim(row.context_id(), row.delta_id()) {
            return Ok(false);
        }
    }

    let mut absorbed = handle.iter::<key::AbsorbBufferKey>()?;
    for record in absorbed.keys() {
        let record = record?;
        if !claim(record.context_id().into(), record.delta_id()) {
            return Ok(false);
        }
    }
    Ok(true)
}
