//! Garbage collection actor for storage tombstones.
//!
//! A delete leaves a tombstone, the only record that the entity was deleted.
//! This actor collects a tombstone once every member device of its context has
//! applied the delete, and never before: a replica that still held the entity,
//! or an older write to it, would otherwise bring it back (core#4331). What
//! "every member has applied it" means, and the evidence for it, is
//! [`crate::tombstone_stability`]'s. Each sweep notes when it first saw each
//! tombstone; a later sweep collects it once every other member device has been
//! caught up with this node since then. There is no time limit: a tombstone of
//! a context with a silent member stays until that member catches up or is
//! removed from the group.
//!
//! A delete tombstones the whole subtree under the deleted entity (every
//! descendant index row is stamped with the same `deleted_at`), so a sweep
//! over the committed keyspace reclaims an entire deleted subtree, not just the
//! directly-deleted row.
//!
//! A delete also lists the deleted entity's id in its parent's
//! `deleted_children`, which exists only to point at the tombstone: the sync
//! wire resolves each id to the child's tombstone and skips one whose tombstone
//! is gone. So once a tombstone is collected, the sweep drops its id from the
//! parent too ([`calimero_storage::reclaim`]), and nothing of the delete is left.
//! Rewriting the parent is a read-modify-write of a row the executor also
//! writes, so a context's reclamation runs under its execution lock.
//!
//! Deleting a row frees no disk space by itself: RocksDB writes a deletion
//! marker, and the old value stays in its SST until a compaction merges the
//! two. So after a sweep, each context the sweep reclaimed enough from has its
//! slice of the state column compacted, on a blocking thread after the
//! reclamation (no lock is held: compaction rewrites files, not rows). See
//! [`GC_COMPACT_MAX_WRITE_AMP`] for when a context qualifies.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::reclaim;
use calimero_storage::store::Key;
use calimero_store::db::Column;
use calimero_store::key::{AsKeyParts, ContextState, STATE_KEY_LEN};
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, error, info, warn};

use crate::tombstone_stability::{
    every_member_caught_up, now_nanos, other_member_devices, TombstoneStability,
};

/// Upper bound on tombstones deleted in a single sweep.
///
/// Bounds both the write amplification and the size of the collected key set
/// per pass, so one sweep can't turn into an unbounded blocking burst on a
/// store with a large tombstone backlog. Anything left over is reclaimed on the
/// next cycle; this cap is only reached under pathological delete volume,
/// which is logged when it happens.
const GC_MAX_DELETIONS_PER_RUN: usize = 10_000;

/// Most bytes a compaction may rewrite per byte the sweep reclaimed.
///
/// Compacting a context's slice rewrites every SST that overlaps it, so its
/// cost tracks the context's size while its benefit tracks what the sweep
/// deleted. A context is compacted once the reclaimed bytes reach 1/32 of its
/// slice: the same order as the write amplification leveled compaction already
/// spends on every byte it stores, so the extra rewrite never costs more than
/// the data's ordinary trip through the levels. Below that the rows are left
/// for background compaction, and the deletion-triggered collector picks up
/// files dense with markers sooner.
const GC_COMPACT_MAX_WRITE_AMP: u64 = 32;

/// Least a context must reclaim in one sweep to be compacted at all (64KiB):
/// a handful of tombstone rows is not worth even a small rewrite.
const GC_COMPACT_MIN_BYTES: u64 = 64 * 1024;

/// Whether deleting `reclaimed` bytes from a slice that takes `size` bytes is
/// worth compacting that slice now: at least [`GC_COMPACT_MIN_BYTES`], and at
/// least 1/[`GC_COMPACT_MAX_WRITE_AMP`] of the slice. The one bar for every
/// sweep that deletes rows and then compacts what it deleted from (this GC's
/// state slices, DAG compaction's delta slices).
pub(crate) fn worth_compacting(reclaimed: u64, size: u64) -> bool {
    reclaimed >= GC_COMPACT_MIN_BYTES && reclaimed.saturating_mul(GC_COMPACT_MAX_WRITE_AMP) >= size
}

/// Message to trigger garbage collection.
#[derive(Copy, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct RunGC;

/// Garbage collector actor for removing expired tombstones.
#[derive(Clone)]
pub struct GarbageCollector {
    /// What one sweep does to the store.
    sweeper: Sweeper,
    /// Hands out each context's execution lock, held while its rows change.
    context_client: ContextClient,
    /// Interval between automatic GC runs.
    interval: Duration,
    /// Guards against a sweep overlapping itself: set while a sweep task is
    /// running, so a tick that fires before the previous sweep finishes (large
    /// store / slow deletes vs. the interval) is skipped rather than racing DB
    /// deletes with an in-flight sweep.
    sweep_in_progress: Arc<AtomicBool>,
}

impl GarbageCollector {
    /// Create a new garbage collector.
    ///
    /// # Arguments
    ///
    /// * `store` - Store handle for accessing the database
    /// * `context_client` - Source of the per-context execution locks
    /// * `stability` - How far each member device has caught up, from beacons
    /// * `interval` - Time between GC runs (default: 1 hour)
    pub(crate) fn new(
        store: Store,
        context_client: ContextClient,
        stability: Arc<TombstoneStability>,
        interval: Duration,
    ) -> Self {
        Self {
            sweeper: Sweeper::new(
                store,
                stability,
                Arc::new(other_member_devices),
                GC_MAX_DELETIONS_PER_RUN,
            ),
            context_client,
            interval,
            sweep_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn one sweep off the actor mailbox unless a previous sweep is still
    /// running. The store work is synchronous RocksDB work, so it runs on
    /// blocking threads (`spawn_blocking`) and never stalls the actor's reactor.
    fn spawn_sweep(&self) {
        if self
            .sweep_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("Skipping GC tick: previous sweep still running");
            return;
        }

        // The in-progress flag is released by an RAII guard shared between
        // this task and every blocking step it spawns, so it clears only once
        // the last of them is done. A `spawn_blocking` task runs to completion
        // even if its `JoinHandle` is dropped (e.g. the actor is torn down
        // mid-sweep), so a step holding its own clone means the flag can never
        // clear while a step is still writing to the store, which would
        // otherwise let a new tick race the in-flight sweep. The guard also
        // covers the normal-completion and panic paths. (If the runtime is
        // shutting down and a blocking step never runs, the flag stays set —
        // benign: no further ticks fire on a stopping actor, and a
        // freshly-constructed actor gets a fresh flag, since `run.rs` builds a
        // new `GarbageCollector` with its own `Arc<AtomicBool>`.)
        let guard = Arc::new(SweepGuard(self.sweep_in_progress.clone()));
        let sweeper = self.sweeper.clone();
        let context_client = self.context_client.clone();
        // Dropping this handle does not cancel the task (Actix/tokio detach it);
        // the `SweepGuard`, not the handle, owns the flag-release invariant.
        let _handle = actix::spawn(async move {
            let start = Instant::now();
            let now = now_nanos();

            let scan = {
                let (sweeper, guard) = (sweeper.clone(), Arc::clone(&guard));
                tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    sweeper.scan(now)
                })
                .await
            };
            let plan = match scan {
                Ok(Ok(plan)) => plan,
                Ok(Err(e)) => return error!(error = ?e, "Garbage collection failed"),
                Err(join_err) => {
                    return error!(error = ?join_err, "Garbage collection task panicked")
                }
            };

            let mut stats = GCStats {
                contexts_scanned: plan.contexts_scanned,
                capped: plan.capped,
                ..GCStats::default()
            };
            // Key and value bytes deleted, per context, for the compaction step.
            let mut reclaimed = BTreeMap::new();
            for (context_id, work) in plan.work {
                // Held across the step, so no execution or sync apply writes
                // this context's rows while they are re-read and rewritten. An
                // unknown context (`None`) has no writer to exclude.
                let lock = context_client.acquire_lock(&context_id).await;
                let (sweeper, guard) = (sweeper.clone(), Arc::clone(&guard));
                let step = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let _lock = lock;
                    sweeper.reclaim(context_id, &work)
                })
                .await;
                match step {
                    Ok(done) => {
                        let _previous = reclaimed.insert(context_id, done.bytes);
                        stats.add(done);
                    }
                    Err(join_err) => {
                        error!(%context_id, error = ?join_err, "Garbage collection step panicked");
                    }
                }
            }
            let compact = tokio::task::spawn_blocking(move || {
                let _guard = guard;
                sweeper.compact_reclaimed(&reclaimed)
            })
            .await;
            match compact {
                Ok(compacted) => stats.contexts_compacted = compacted,
                Err(join_err) => {
                    error!(error = ?join_err, "Garbage collection compaction panicked")
                }
            }
            stats.duration_ms = start.elapsed().as_millis() as u64;
            stats.log();
        });
    }
}

/// The member devices of a context that must be caught up before one of its
/// tombstones goes, other than this node's own
/// ([`other_member_devices`] in production).
type MemberDevices = dyn Fn(&Store, &ContextId) -> EyreResult<Vec<PublicKey>> + Send + Sync;

/// When a sweep first saw a tombstone, and the `deleted_at` it carried then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    deleted_at: u64,
    at: u64,
}

/// One sweep's work on the store, without the locking and scheduling around it.
#[derive(Clone)]
struct Sweeper {
    /// Store handle for database access.
    store: Store,
    /// How far each member device has caught up with this node.
    stability: Arc<TombstoneStability>,
    /// Who must be caught up, per context.
    members: Arc<MemberDevices>,
    /// Every tombstone the last scan found, with when it was first seen. A
    /// tombstone goes only once every member is caught up as of after that, so
    /// the first scan to see one never collects it.
    seen: Arc<Mutex<BTreeMap<(ContextId, Id), Seen>>>,
    /// Max tombstones deleted per sweep (see [`GC_MAX_DELETIONS_PER_RUN`]).
    max_deletions_per_run: usize,
}

/// The rows of one context a sweep found work on.
#[derive(Debug, Default)]
struct ContextWork {
    /// Tombstones every member has caught up past, to delete, with the
    /// `deleted_at` they carried when the scan judged them.
    tombstones: Vec<(ContextState, u64)>,
    /// Rows listing deleted children, whose collected ones to drop.
    parents: Vec<ContextState>,
}

/// What a scan found, by context.
#[derive(Debug, Default)]
struct Plan {
    work: BTreeMap<ContextId, ContextWork>,
    /// Number of distinct contexts observed during the scan.
    contexts_scanned: usize,
    /// Whether the scan stopped early at the per-run deletion cap.
    capped: bool,
}

/// What reclaiming one context's rows did.
#[derive(Debug, Default, Clone, Copy)]
struct Reclaimed {
    tombstones_collected: usize,
    parents_pruned: usize,
    /// Key and value bytes of the deleted tombstones.
    bytes: u64,
}

impl Sweeper {
    /// Construct with explicit members and cap, so tests can name who must be
    /// caught up without a governance store.
    fn new(
        store: Store,
        stability: Arc<TombstoneStability>,
        members: Arc<MemberDevices>,
        max_deletions_per_run: usize,
    ) -> Self {
        Self {
            store,
            stability,
            members,
            seen: Arc::default(),
            max_deletions_per_run,
        }
    }

    /// Single pass over the committed `ContextState` keyspace: find every
    /// tombstone every member device has caught up past, up to
    /// `max_deletions_per_run`, and every other row that lists deleted
    /// children. Records when it first saw each tombstone.
    ///
    /// One scan covers all contexts (the column is keyed by
    /// `(context_id, state_key)`), so this is O(total state keys) rather than
    /// O(contexts × total state keys). It changes nothing: [`Self::reclaim`]
    /// re-reads every row it acts on.
    ///
    /// `now_nanos` is when this scan sees what it finds; tests inject it.
    fn scan(&self, now_nanos: u64) -> EyreResult<Plan> {
        let mut iter = self.store.iter::<ContextState>()?;
        let mut plan = Plan::default();
        let mut tombstones = 0usize;
        let previously_seen = std::mem::take(&mut *self.lock_seen());
        let mut seen = BTreeMap::new();
        // Who must be caught up, per context, read once per scan. `None` when
        // it cannot be read: that context collects nothing this pass.
        let mut members: BTreeMap<ContextId, Option<Vec<PublicKey>>> = BTreeMap::new();

        // Count distinct contexts (a log-only metric) in O(1) memory: the column
        // is keyed `context_id ‖ state_key`, so entries iterate grouped by
        // context and a change of `context_id` marks a new context. If iteration
        // order ever differed this would only over-count the metric, never affect
        // reclamation.
        let mut last_context = None;

        while let Some(entry) = iter.next()? {
            let context_id = entry.context_id();
            if last_context != Some(context_id) {
                plan.contexts_scanned += 1;
                last_context = Some(context_id);
            }

            // The iterator's key snapshot and this per-key `get` are not atomic;
            // a concurrent writer could change the keyspace mid-scan. Every
            // outcome is safe: what the scan finds is only a candidate, which
            // `reclaim` re-validates against the current row under the
            // context's lock, and a row written past the cursor is simply
            // missed this pass — harmless, since a tombstone is never collected
            // by the scan that first sees it, and the next sweep catches it (GC
            // is eventually consistent).
            // Only entity rows can be tombstones or list children, and the key
            // says which rows those are; everything else is skipped without a
            // read.
            let Some(id) = entity_id(&entry) else {
                continue;
            };
            let Some(value) = self.store.get(&entry)? else {
                continue;
            };

            if let Some(deleted_at) = reclaim::tombstone_deleted_at(id, value.as_ref()) {
                // First seen now, unless the last scan saw this same delete. A
                // newer delete of the entity (a raised `deleted_at`) starts
                // over: the members have to catch up past it too.
                let first_seen = previously_seen
                    .get(&(context_id, id))
                    .filter(|seen| seen.deleted_at == deleted_at)
                    .map(|seen| seen.at);
                let at = first_seen.unwrap_or(now_nanos);
                let _previous = seen.insert((context_id, id), Seen { deleted_at, at });
                if first_seen.is_none() || plan.capped {
                    continue;
                }
                let members = members.entry(context_id).or_insert_with(|| {
                    (self.members)(&self.store, &context_id)
                        .inspect_err(|e| {
                            warn!(%context_id, error = ?e, "GC could not read a context's members; collecting nothing there");
                        })
                        .ok()
                });
                let Some(members) = members else {
                    continue;
                };
                let caught_up = |member: &PublicKey| self.stability.caught_up(context_id, member);
                if !every_member_caught_up(members, caught_up, at) {
                    continue;
                }
                plan.work
                    .entry(context_id)
                    .or_default()
                    .tombstones
                    .push((entry, deleted_at));
                tombstones += 1;
                if tombstones >= self.max_deletions_per_run {
                    // Keep scanning, to note when every other tombstone was
                    // first seen, but plan no more deletes.
                    plan.capped = true;
                }
            } else if reclaim::lists_deleted_children(id, value.as_ref()) {
                plan.work.entry(context_id).or_default().parents.push(entry);
            }
        }

        *self.lock_seen() = seen;
        Ok(plan)
    }

    fn lock_seen(&self) -> std::sync::MutexGuard<'_, BTreeMap<(ContextId, Id), Seen>> {
        // Poisoned only by a panic mid-scan; the map is then at worst emptied,
        // which delays collection and never hastens it.
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reclaim what [`Self::scan`] found in `context_id`: delete the
    /// tombstones every member has caught up past, then drop every collected
    /// child from the deleted children its parents list. Runs under the
    /// context's execution lock.
    ///
    /// Best-effort: a single failed read or write must not abort the step and
    /// strand the rest. Counts are actual changes (work done), not intents;
    /// leftovers retry next cycle. Each change is an independent atomic store
    /// write, so an interrupted step leaves the store consistent: a parent
    /// still listing a collected child is what the store held before this
    /// change, and the sync wire already skips such an entry.
    fn reclaim(&self, context_id: ContextId, work: &ContextWork) -> Reclaimed {
        let mut store = self.store.clone();
        let mut done = Reclaimed::default();

        for (key, deleted_at) in &work.tombstones {
            // Re-validate against the CURRENT value right before deleting. The
            // scan read this key earlier, and the entity may have been
            // resurrected since (tombstone → live re-add), or deleted again
            // (a newer `deleted_at`, which the members have not been seen past
            // yet). Deleting only the very tombstone the scan judged means GC
            // never removes a resurrected live row, nor a newer delete.
            let Some(id) = entity_id(key) else {
                continue;
            };
            let row_bytes = match self.store.get(key) {
                Ok(Some(value)) => (reclaim::tombstone_deleted_at(id, value.as_ref())
                    == Some(*deleted_at))
                .then(|| (key.as_key().as_bytes().len() + value.len()) as u64),
                Ok(None) => None, // already gone
                Err(e) => {
                    warn!(error = ?e, "GC failed to re-read a tombstone; will retry next cycle");
                    continue;
                }
            };
            let Some(row_bytes) = row_bytes else {
                continue;
            };

            match store.delete(key) {
                Ok(()) => {
                    done.tombstones_collected += 1;
                    done.bytes += row_bytes;
                }
                Err(e) => {
                    warn!(error = ?e, "GC failed to delete a tombstone; will retry next cycle");
                }
            }
        }

        for key in &work.parents {
            let Some(id) = entity_id(key) else {
                continue;
            };
            let value = match self.store.get(key) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(e) => {
                    warn!(error = ?e, "GC failed to re-read a parent; will retry next cycle");
                    continue;
                }
            };
            // A child whose row cannot be read is treated as still present:
            // the safe direction keeps its entry for the next cycle.
            let has_row = |child: Id| {
                self.store
                    .has(&ContextState::new(context_id, Key::Index(child).to_bytes()))
                    .unwrap_or(true)
            };
            let Some(pruned) = reclaim::prune_deleted_children(id, value.as_ref(), has_row) else {
                continue;
            };
            match store.put(key, Slice::from(pruned)) {
                Ok(()) => done.parents_pruned += 1,
                Err(e) => {
                    warn!(error = ?e, "GC failed to rewrite a parent; will retry next cycle");
                }
            }
        }

        done
    }

    /// Compact the state-column slice of every context whose reclaimed bytes
    /// clear [`GC_COMPACT_MIN_BYTES`] and 1/[`GC_COMPACT_MAX_WRITE_AMP`] of
    /// the slice's size. Returns how many were compacted.
    ///
    /// Best-effort like the deletes: a failure is logged and the context's
    /// space is left to background compaction.
    fn compact_reclaimed(&self, reclaimed: &BTreeMap<ContextId, u64>) -> usize {
        let mut compacted = 0;
        for (&context_id, &bytes) in reclaimed {
            let lo = ContextState::new(context_id, [0; STATE_KEY_LEN]);
            let hi = ContextState::new(context_id, [u8::MAX; STATE_KEY_LEN]);
            let (lo, hi) = (lo.as_key().as_bytes(), hi.as_key().as_bytes());
            let size = match self.store.approximate_size(Column::State, lo, hi) {
                Ok(size) => size,
                Err(e) => {
                    warn!(%context_id, error = ?e, "GC could not size a context for compaction");
                    continue;
                }
            };
            if !worth_compacting(bytes, size) {
                continue;
            }
            let t = Instant::now();
            match self.store.raw_compact_range(Column::State, lo, hi) {
                Ok(()) => {
                    compacted += 1;
                    debug!(%context_id, reclaimed = bytes, size, took = ?t.elapsed(), "GC compacted a context's state");
                }
                Err(e) => warn!(%context_id, error = ?e, "GC failed to compact a context's state"),
            }
        }
        compacted
    }

    /// A whole sweep without locks, as tests drive it.
    #[cfg(test)]
    fn sweep(&self, now_nanos: u64) -> EyreResult<GCStats> {
        let plan = self.scan(now_nanos)?;
        let mut stats = GCStats {
            contexts_scanned: plan.contexts_scanned,
            capped: plan.capped,
            ..GCStats::default()
        };
        let mut reclaimed = BTreeMap::new();
        for (context_id, work) in plan.work {
            let done = self.reclaim(context_id, &work);
            let _previous = reclaimed.insert(context_id, done.bytes);
            stats.add(done);
        }
        stats.contexts_compacted = self.compact_reclaimed(&reclaimed);
        Ok(stats)
    }
}

/// The entity a `ContextState` key holds the row of, or `None` for any other
/// kind of state row (child trie, sync state).
fn entity_id(key: &ContextState) -> Option<Id> {
    match Key::from_bytes(&key.state_key())? {
        Key::Index(id) => Some(id),
        _ => None,
    }
}

impl Actor for GarbageCollector {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            interval_secs = self.interval.as_secs(),
            max_deletions_per_run = self.sweeper.max_deletions_per_run,
            "Garbage collection actor started"
        );

        // Sweep once on startup, so the tombstones a restart left behind are
        // noted at once and can go at the next sweep.
        self.spawn_sweep();

        // Schedule periodic GC runs.
        let interval = self.interval;
        let _handle = ctx.run_interval(interval, |act, _ctx| act.spawn_sweep());
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("Garbage collection actor stopped");
    }
}

impl Handler<RunGC> for GarbageCollector {
    type Result = ();

    fn handle(&mut self, _msg: RunGC, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Starting garbage collection cycle");
        self.spawn_sweep();
    }
}

/// Releases the sweep-in-progress flag on drop.
///
/// Shared by the spawned sweep task and each blocking step it runs, so the
/// flag is cleared on completion AND on cancellation (the task being dropped,
/// e.g. actor teardown), once the last step is done. Without this, a cancelled
/// sweep would leave the flag set and permanently skip every later tick.
struct SweepGuard(Arc<AtomicBool>);

impl Drop for SweepGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Statistics from a garbage collection run.
#[derive(Debug, Default)]
struct GCStats {
    /// Number of tombstones collected.
    tombstones_collected: usize,
    /// Number of parent rows rewritten without their collected children.
    parents_pruned: usize,
    /// Number of distinct contexts observed during the sweep.
    contexts_scanned: usize,
    /// Number of contexts whose state slice was compacted after the deletes.
    contexts_compacted: usize,
    /// Duration of the GC run in milliseconds.
    duration_ms: u64,
    /// Whether the sweep stopped early at the per-run deletion cap.
    capped: bool,
}

impl GCStats {
    fn add(&mut self, done: Reclaimed) {
        self.tombstones_collected += done.tombstones_collected;
        self.parents_pruned += done.parents_pruned;
    }

    fn log(&self) {
        if self.tombstones_collected > 0 || self.parents_pruned > 0 || self.capped {
            info!(
                tombstones_collected = self.tombstones_collected,
                parents_pruned = self.parents_pruned,
                contexts_scanned = self.contexts_scanned,
                contexts_compacted = self.contexts_compacted,
                duration_ms = self.duration_ms,
                capped = self.capped,
                "Garbage collection completed"
            );
        }
        if self.capped {
            warn!(
                collected = self.tombstones_collected,
                "GC hit the per-run deletion cap; remaining collectable \
                 tombstones will be reclaimed on the next cycle"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{EntryRules, StorageType};
    use calimero_storage::index::EntityIndex;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::ContextState as ContextStateKey;
    use calimero_store::layer::{ReadLayer, WriteLayer};
    use calimero_store::slice::Slice;
    use calimero_store::Store;

    use super::*;

    use crate::tombstone_stability::SETTLE_NANOS;

    const DAY_NANOS: u64 = 86_400_000_000_000;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A sweeper for a context with no other member: every tombstone goes on
    /// the sweep after the one that first sees it.
    fn gc(store: Store, cap: usize) -> Sweeper {
        with_members(store, cap, Vec::new(), Arc::default())
    }

    /// A sweeper whose every context has `members` as its other devices.
    fn with_members(
        store: Store,
        cap: usize,
        members: Vec<PublicKey>,
        stability: Arc<TombstoneStability>,
    ) -> Sweeper {
        Sweeper::new(
            store,
            stability,
            Arc::new(move |_: &Store, _: &ContextId| Ok(members.clone())),
            cap,
        )
    }

    /// Two sweeps: the first notes every tombstone, the second collects what
    /// may go. Returns the second's stats.
    fn sweep_twice(sweeper: &Sweeper, now: u64) -> GCStats {
        let _first = sweeper.sweep(now).unwrap();
        sweeper.sweep(now).unwrap()
    }

    /// `member` shows, at `at`, a state equal to this node's at `at`.
    fn caught_up(stability: &TombstoneStability, ctx: ContextId, member: PublicKey, at: u64) {
        let (heads, root) = (vec![[at as u8; 32]], [at as u8; 32]);
        stability.record_own(ctx, heads.clone(), root, at);
        assert!(stability.observe(ctx, member, &heads, root));
    }

    /// Writes the entity row of `id`: an `EntityIndex` carrying `deleted_at`,
    /// and no data. Returns the key so tests can assert on its presence.
    fn put_index_row(
        store: &Store,
        ctx: ContextId,
        id: [u8; 32],
        deleted_at: Option<u64>,
    ) -> ContextStateKey {
        let mut index = EntityIndex::minimal_for_test(Id::new(id));
        index.deleted_at = deleted_at;
        let bytes = index_row(&index);

        let key = ContextStateKey::new(ctx, entity_key(index.id()));
        let mut handle = store.clone();
        handle.put(&key, Slice::from(bytes)).unwrap();
        key
    }

    /// The state key of entity `id`'s row.
    fn entity_key(id: Id) -> [u8; calimero_store::key::STATE_KEY_LEN] {
        calimero_storage::store::Key::Index(id).to_bytes()
    }

    /// The entity row holding `index` and no data, as the storage layer
    /// stores a tombstone.
    fn index_row(index: &EntityIndex) -> Vec<u8> {
        calimero_storage::row::encode(
            index.id(),
            &calimero_storage::row::Row {
                index: Some(borsh::to_vec(index).unwrap()),
                data: None,
            },
        )
    }

    fn exists(store: &Store, key: &ContextStateKey) -> bool {
        store.get(key).unwrap().is_some()
    }

    /// A sweep that panics mid-run must still clear `sweep_in_progress` so the
    /// next tick is not permanently skipped. The flag lives in a `SweepGuard`
    /// held inside the sweep task; its `Drop` runs on unwind, so catching a
    /// simulated panic must leave the flag cleared.
    #[test]
    fn panicking_sweep_clears_in_progress_flag() {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Enter a sweep: claim the flag exactly as `spawn_sweep` does.
        assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Claim the flag BEFORE constructing the guard, matching production
            // (`spawn_sweep` sets it, then builds the guard). Setting it after —
            // or not at all — would leave it `false`, so the final assertion
            // would pass even with a no-op `Drop`; setting it first makes the
            // assertion actually prove the guard cleared it.
            flag.store(true, std::sync::atomic::Ordering::Release);
            let _guard = SweepGuard(Arc::clone(&flag));
            // A sweep aborting mid-run (e.g. a store error surfacing as a panic).
            panic!("simulated sweep panic");
        }));
        assert!(result.is_err(), "the sweep must have unwound");

        // The guard's Drop ran during unwind, so a later tick can start.
        assert!(
            !flag.load(std::sync::atomic::Ordering::Acquire),
            "SweepGuard must clear the in-progress flag even when the sweep panics"
        );
    }

    /// A delete stamps every descendant index row with the same `deleted_at`, so
    /// a sweep reclaims the whole tombstoned subtree — while a still-live row
    /// (no `deleted_at`) survives, and so does every tombstone on the first
    /// sweep that sees it.
    #[test]
    fn reclaims_all_tombstoned_subtree_rows() {
        let store = store();
        let ctx = ContextId::from([1u8; 32]);
        let deleted_at = 10 * DAY_NANOS;

        // Three subtree rows tombstoned at the same instant (as a subtree delete
        // stamps them).
        let a = put_index_row(&store, ctx, [10u8; 32], Some(deleted_at));
        let b = put_index_row(&store, ctx, [11u8; 32], Some(deleted_at));
        let c = put_index_row(&store, ctx, [12u8; 32], Some(deleted_at));
        let live = put_index_row(&store, ctx, [13u8; 32], None);
        let sweeper = gc(store.clone(), GC_MAX_DELETIONS_PER_RUN);

        let first = sweeper.sweep(deleted_at).unwrap();
        assert_eq!(
            first.tombstones_collected, 0,
            "the first sweep only notes them"
        );
        assert!(exists(&store, &a));

        let second = sweeper.sweep(deleted_at).unwrap();
        assert_eq!(second.tombstones_collected, 3);
        assert!(!exists(&store, &a));
        assert!(!exists(&store, &b));
        assert!(!exists(&store, &c));
        assert!(exists(&store, &live), "live row must survive");
    }

    /// The case behind core#4331: a member that never catches up, as a replica
    /// offline for any length of time does not, keeps every tombstone, however
    /// old. Collected, it would let that replica bring the entities back.
    #[test]
    fn a_member_that_never_catches_up_keeps_the_tombstone() {
        let store = store();
        let ctx = ContextId::from([15u8; 32]);
        let offline = PublicKey::from([0xAA; 32]);
        let key = put_index_row(&store, ctx, [16u8; 32], Some(DAY_NANOS));
        let sweeper = with_members(
            store.clone(),
            GC_MAX_DELETIONS_PER_RUN,
            vec![offline],
            Arc::default(),
        );

        for day in 1..=365 {
            let stats = sweeper.sweep(day * DAY_NANOS).unwrap();
            assert_eq!(stats.tombstones_collected, 0);
        }
        assert!(exists(&store, &key));
    }

    /// A tombstone goes once every member has caught up, as of a moment
    /// [`SETTLE_NANOS`] after the sweep first saw it, and not before: a member
    /// caught up earlier, or another still behind, keeps it.
    #[test]
    fn collects_once_every_member_has_caught_up_past_it() {
        let store = store();
        let ctx = ContextId::from([17u8; 32]);
        let (x, y) = (PublicKey::from([0xA1; 32]), PublicKey::from([0xB2; 32]));
        let stability: Arc<TombstoneStability> = Arc::default();
        let key = put_index_row(&store, ctx, [18u8; 32], Some(DAY_NANOS));
        let sweeper = with_members(
            store.clone(),
            GC_MAX_DELETIONS_PER_RUN,
            vec![x, y],
            Arc::clone(&stability),
        );
        let seen_at = 2 * DAY_NANOS;
        let _first = sweeper.sweep(seen_at).unwrap();

        // Caught up before the settle margin is over: not proof of the delete.
        caught_up(&stability, ctx, x, seen_at + SETTLE_NANOS - 1);
        caught_up(&stability, ctx, y, seen_at + SETTLE_NANOS - 1);
        assert_eq!(
            sweeper.sweep(3 * DAY_NANOS).unwrap().tombstones_collected,
            0
        );

        // One member past it, the other not yet.
        caught_up(&stability, ctx, x, seen_at + SETTLE_NANOS);
        assert_eq!(
            sweeper.sweep(3 * DAY_NANOS).unwrap().tombstones_collected,
            0
        );
        assert!(exists(&store, &key));

        caught_up(&stability, ctx, y, seen_at + SETTLE_NANOS);
        assert_eq!(
            sweeper.sweep(3 * DAY_NANOS).unwrap().tombstones_collected,
            1
        );
        assert!(!exists(&store, &key));
    }

    /// A newer delete of the same entity (a raised `deleted_at`) starts the
    /// wait over: members caught up past the old one may still hold the
    /// write it superseded.
    #[test]
    fn a_newer_delete_waits_again() {
        let store = store();
        let ctx = ContextId::from([19u8; 32]);
        let x = PublicKey::from([0xC3; 32]);
        let stability: Arc<TombstoneStability> = Arc::default();
        let _key = put_index_row(&store, ctx, [20u8; 32], Some(DAY_NANOS));
        let sweeper = with_members(
            store.clone(),
            GC_MAX_DELETIONS_PER_RUN,
            vec![x],
            Arc::clone(&stability),
        );
        let _first = sweeper.sweep(DAY_NANOS).unwrap();
        caught_up(&stability, ctx, x, DAY_NANOS + SETTLE_NANOS);

        // Deleted again, after the member was seen.
        let key = put_index_row(&store, ctx, [20u8; 32], Some(2 * DAY_NANOS));
        assert_eq!(
            sweeper.sweep(3 * DAY_NANOS).unwrap().tombstones_collected,
            0
        );
        assert!(exists(&store, &key));

        caught_up(&stability, ctx, x, 3 * DAY_NANOS + SETTLE_NANOS);
        assert_eq!(
            sweeper.sweep(4 * DAY_NANOS).unwrap().tombstones_collected,
            1
        );
        assert!(!exists(&store, &key));
    }

    /// A context whose members cannot be read collects nothing.
    #[test]
    fn unreadable_members_collect_nothing() {
        let store = store();
        let ctx = ContextId::from([21u8; 32]);
        let key = put_index_row(&store, ctx, [22u8; 32], Some(DAY_NANOS));
        let sweeper = Sweeper::new(
            store.clone(),
            Arc::default(),
            Arc::new(|_: &Store, _: &ContextId| Err(eyre::eyre!("unreadable"))),
            GC_MAX_DELETIONS_PER_RUN,
        );

        assert_eq!(sweep_twice(&sweeper, DAY_NANOS).tombstones_collected, 0);
        assert!(exists(&store, &key));
    }

    /// The tombstone of a deleted written-once entry is the record that keeps
    /// its owner's key deleted, so no sweep collects it, however old.
    #[test]
    fn never_reclaims_a_written_once_entry_s_tombstone() {
        let store = store();
        let ctx = ContextId::from([4u8; 32]);
        let deleted_at = 10 * DAY_NANOS;
        let owned = |immutable: bool| {
            let mut index = EntityIndex::minimal_for_test(Id::new([40 + u8::from(immutable); 32]));
            index.deleted_at = Some(deleted_at);
            index.metadata.storage_type = StorageType::User {
                owner: [7; 32].into(),
                rules: EntryRules {
                    immutable,
                    moderators: None,
                },
                signature_data: None,
            };
            let key = ContextStateKey::new(ctx, entity_key(index.id()));
            let mut handle = store.clone();
            handle.put(&key, Slice::from(index_row(&index))).unwrap();
            key
        };
        let written_once = owned(true);
        let editable = owned(false);

        let stats = sweep_twice(&gc(store.clone(), GC_MAX_DELETIONS_PER_RUN), deleted_at);

        assert_eq!(stats.tombstones_collected, 1);
        assert!(exists(&store, &written_once));
        assert!(!exists(&store, &editable));
    }

    /// A collected tombstone takes its id in its parent's `deleted_children`
    /// with it, in the same sweep: the id only points at the tombstone. A child
    /// whose tombstone stays (here, one deleted again since the last sweep)
    /// stays listed. A child with no row at all (e.g. a parent that arrived by snapshot, which ships no
    /// tombstones) is inert already, so it goes too. The parent keeps its data
    /// and every other field.
    #[test]
    fn collecting_a_tombstone_drops_it_from_its_parent() {
        let store = store();
        let ctx = ContextId::from([7u8; 32]);
        let deleted_at = 10 * DAY_NANOS;
        let now = deleted_at + 2 * DAY_NANOS;

        let expired = put_index_row(&store, ctx, [70u8; 32], Some(deleted_at));
        let _ = put_index_row(&store, ctx, [71u8; 32], Some(deleted_at));
        let never_held = Id::new([72u8; 32]);

        let parent_id = Id::new([79u8; 32]);
        let mut parent = EntityIndex::minimal_for_test(parent_id);
        parent.deleted_children = vec![Id::new([70u8; 32]), Id::new([71u8; 32]), never_held];
        let data = b"collection bytes".to_vec();
        let parent_key = ContextStateKey::new(ctx, entity_key(parent_id));
        let row = |index: &EntityIndex| {
            calimero_storage::row::encode(
                parent_id,
                &calimero_storage::row::Row {
                    index: Some(borsh::to_vec(index).unwrap()),
                    data: Some(data.clone()),
                },
            )
        };
        let mut handle = store.clone();
        handle.put(&parent_key, Slice::from(row(&parent))).unwrap();
        let sweeper = gc(store.clone(), GC_MAX_DELETIONS_PER_RUN);
        let _first = sweeper.sweep(now).unwrap();
        // Deleted again after the first sweep, so the second one only notes it.
        let recent = put_index_row(&store, ctx, [71u8; 32], Some(now));

        let stats = sweeper.sweep(now).unwrap();

        assert_eq!(stats.tombstones_collected, 1);
        assert_eq!(stats.parents_pruned, 1);
        assert!(!exists(&store, &expired));
        assert!(exists(&store, &recent));
        let mut expected = parent.clone();
        expected.deleted_children = vec![Id::new([71u8; 32])];
        assert_eq!(
            store.get(&parent_key).unwrap().unwrap().as_ref(),
            row(&expected).as_slice(),
            "the parent must lose exactly the collected and absent children"
        );

        // Nothing left to drop while `recent` stays: a fresh sweeper, which
        // has seen nothing yet, rewrites nothing.
        let again = gc(store.clone(), GC_MAX_DELETIONS_PER_RUN)
            .sweep(now)
            .unwrap();
        assert_eq!(again.parents_pruned, 0);
    }

    /// A written-once entry's tombstone is kept for good, and so is its id in
    /// its parent: the sync wire ships that tombstone through the list.
    #[test]
    fn keeps_a_terminal_tombstone_listed() {
        let store = store();
        let ctx = ContextId::from([8u8; 32]);
        let child = Id::new([80u8; 32]);
        let mut index = EntityIndex::minimal_for_test(child);
        index.deleted_at = Some(1);
        index.metadata.storage_type = StorageType::User {
            owner: [7; 32].into(),
            rules: EntryRules {
                immutable: true,
                moderators: None,
            },
            signature_data: None,
        };
        let mut handle = store.clone();
        handle
            .put(
                &ContextStateKey::new(ctx, entity_key(child)),
                Slice::from(index_row(&index)),
            )
            .unwrap();
        let mut parent = EntityIndex::minimal_for_test(Id::new([89u8; 32]));
        parent.deleted_children = vec![child];
        let parent_key = ContextStateKey::new(ctx, entity_key(parent.id()));
        handle
            .put(&parent_key, Slice::from(index_row(&parent)))
            .unwrap();

        let stats = sweep_twice(&gc(store.clone(), GC_MAX_DELETIONS_PER_RUN), DAY_NANOS);

        assert_eq!(stats.tombstones_collected, 0);
        assert_eq!(stats.parents_pruned, 0);
        assert_eq!(
            store.get(&parent_key).unwrap().unwrap().as_ref(),
            index_row(&parent).as_slice()
        );
    }

    /// An interrupted sweep (modelled by the per-run cap firing mid-pass) leaves
    /// the store consistent — the undeleted tombstones remain valid — and
    /// subsequent passes complete the reclamation.
    #[test]
    fn interrupted_sweep_is_consistent_and_next_pass_completes() {
        let store = store();
        let ctx = ContextId::from([2u8; 32]);
        let deleted_at = 10 * DAY_NANOS;
        let now = deleted_at + 2 * DAY_NANOS;

        let keys: Vec<_> = (0..3)
            .map(|i| put_index_row(&store, ctx, [20 + i as u8; 32], Some(deleted_at)))
            .collect();

        // Cap of 1 stops the sweep after a single delete, as an interrupt would.
        let collector = gc(store.clone(), 1);

        let _noted = collector.sweep(now).unwrap();
        let first = collector.sweep(now).unwrap();
        assert_eq!(first.tombstones_collected, 1);
        assert!(first.capped);
        // Store still consistent: exactly two tombstones remain and are readable.
        assert_eq!(keys.iter().filter(|k| exists(&store, k)).count(), 2);

        // Resume: further passes finish the work.
        let _ = collector.sweep(now).unwrap();
        let _ = collector.sweep(now).unwrap();
        assert_eq!(keys.iter().filter(|k| exists(&store, k)).count(), 0);
    }

    /// Non-index values (entity data blobs) are never reclaimed: they don't
    /// pass the `EntityIndex` round-trip guard.
    #[test]
    fn non_index_values_are_never_reclaimed() {
        let store = store();
        let ctx = ContextId::from([5u8; 32]);

        let key = ContextStateKey::new(ctx, entity_key(Id::new([50u8; 32])));
        let mut handle = store.clone();
        handle.put(&key, Slice::from(vec![0xABu8; 128])).unwrap();

        let stats = sweep_twice(&gc(store.clone(), GC_MAX_DELETIONS_PER_RUN), DAY_NANOS);

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key), "entity data must never be reclaimed");
    }

    /// A row under another kind's key is never reclaimed, even when its bytes
    /// are a valid tombstone: only entity keys are candidates.
    #[test]
    fn rows_under_other_key_kinds_are_never_reclaimed() {
        let store = store();
        let ctx = ContextId::from([6u8; 32]);
        let id = Id::new([51u8; 32]);
        let mut index = EntityIndex::minimal_for_test(id);
        index.deleted_at = Some(1);
        let key = ContextStateKey::new(ctx, calimero_storage::store::Key::ChildTrie(id).to_bytes());
        let mut handle = store.clone();
        handle.put(&key, Slice::from(index_row(&index))).unwrap();

        let stats = sweep_twice(&gc(store.clone(), GC_MAX_DELETIONS_PER_RUN), DAY_NANOS);

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key));
    }

    /// On RocksDB a sweep gives the space back: the context it reclaimed most
    /// of is compacted, so its slice shrinks instead of growing by the deletion
    /// markers, while a context that reclaimed only a few rows is left alone.
    #[test]
    fn sweep_compacts_the_contexts_it_reclaimed_from() {
        use calimero_store::config::StoreConfig;
        use calimero_store_rocksdb::RocksDB;

        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let store = Store::open::<RocksDB>(&StoreConfig::new(path)).unwrap();
        let deleted_at = 10 * DAY_NANOS;
        let id = |ctx: u8, i: u32| {
            let mut id = [ctx; 32];
            id[..4].copy_from_slice(&i.to_be_bytes());
            id
        };
        let slice_size = |ctx: ContextId| {
            store.flush().unwrap();
            let lo = ContextStateKey::new(ctx, [0; STATE_KEY_LEN]);
            let hi = ContextStateKey::new(ctx, [u8::MAX; STATE_KEY_LEN]);
            store
                .approximate_size(
                    Column::State,
                    lo.as_key().as_bytes(),
                    hi.as_key().as_bytes(),
                )
                .unwrap()
        };

        // `busy` is almost all tombstones; `quiet` holds a few among live rows.
        let busy = ContextId::from([1u8; 32]);
        let quiet = ContextId::from([2u8; 32]);
        for i in 0..8_000 {
            let _ = put_index_row(&store, busy, id(1, i), Some(deleted_at));
        }
        for i in 0..100 {
            let _ = put_index_row(&store, busy, id(3, i), None);
            let _ = put_index_row(&store, quiet, id(2, i), Some(deleted_at));
        }
        for i in 0..2_000 {
            let _ = put_index_row(&store, quiet, id(4, i), None);
        }
        let busy_before = slice_size(busy);

        let stats = sweep_twice(&gc(store.clone(), GC_MAX_DELETIONS_PER_RUN), deleted_at);

        assert_eq!(stats.tombstones_collected, 8_100);
        assert_eq!(stats.contexts_compacted, 1, "only `busy` clears the bar");
        let busy_after = slice_size(busy);
        assert!(
            busy_after * 10 < busy_before,
            "compaction must give the reclaimed rows back: {busy_after} of {busy_before} bytes left"
        );
    }

    /// `tombstone_deleted_at`'s index-vs-data guard assumes `EntityIndex` borsh
    /// is canonical — re-serializing a decoded value yields identical bytes.
    /// Lock that invariant so a future field of a non-canonical type (e.g. a
    /// `HashMap`) is caught here rather than silently weakening the guard.
    #[test]
    fn entity_index_borsh_roundtrips() {
        let mut index = EntityIndex::minimal_for_test(Id::new([9u8; 32]));
        index.deleted_at = Some(123);
        let bytes = borsh::to_vec(&index).unwrap();

        let decoded = borsh::from_slice::<EntityIndex>(&bytes).unwrap();
        assert_eq!(
            borsh::to_vec(&decoded).unwrap(),
            bytes,
            "EntityIndex borsh must be canonical for the round-trip guard to hold"
        );
        assert_eq!(
            reclaim::tombstone_deleted_at(index.id(), &index_row(&decoded)),
            Some(123)
        );
    }
}
