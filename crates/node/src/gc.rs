//! Garbage collection actor for storage tombstones.
//!
//! This module provides automatic cleanup of old tombstones in the storage layer.
//! Tombstones are created when entities are deleted in the CRDT storage system,
//! and this actor periodically removes tombstones that have exceeded their
//! retention period.
//!
//! A delete tombstones the whole subtree under the deleted entity (every
//! descendant index row is stamped with the same `deleted_at`), so a single
//! sweep over the committed keyspace reclaims an entire deleted subtree once its
//! retention elapses — not just the directly-deleted row.
//!
//! A delete also lists the deleted entity's id in its parent's
//! `deleted_children`, which exists only to point at the tombstone: the sync
//! wire resolves each id to the child's tombstone and skips one whose tombstone
//! is gone. So once a tombstone is collected, the sweep drops its id from the
//! parent too ([`calimero_storage::reclaim`]), and nothing of the delete is left.
//! Rewriting the parent is a read-modify-write of a row the executor also
//! writes, so a context's reclamation runs under its execution lock.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
use calimero_storage::reclaim;
use calimero_storage::store::Key;
use calimero_store::key::ContextState;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, error, info, warn};

/// Upper bound on tombstones deleted in a single sweep.
///
/// Bounds both the write amplification and the size of the collected key set
/// per pass, so one sweep can't turn into an unbounded blocking burst on a
/// store with a large tombstone backlog. Anything left over is reclaimed on the
/// next cycle; with a one-day retention and a twelve-hour cadence this cap is
/// only reached under pathological delete volume, which is logged when it
/// happens.
const GC_MAX_DELETIONS_PER_RUN: usize = 10_000;

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
    /// * `interval` - Time between GC runs (default: 12 hours)
    pub fn new(store: Store, context_client: ContextClient, interval: Duration) -> Self {
        Self {
            sweeper: Sweeper::new(store, TOMBSTONE_RETENTION_NANOS, GC_MAX_DELETIONS_PER_RUN),
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
            for (context_id, work) in plan.work {
                // Held across the step, so no execution or sync apply writes
                // this context's rows while they are re-read and rewritten. An
                // unknown context (`None`) has no writer to exclude.
                let lock = context_client.acquire_lock(&context_id).await;
                let (sweeper, guard) = (sweeper.clone(), Arc::clone(&guard));
                let step = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let _lock = lock;
                    sweeper.reclaim(context_id, &work, now)
                })
                .await;
                match step {
                    Ok(done) => stats.add(done),
                    Err(join_err) => {
                        error!(%context_id, error = ?join_err, "Garbage collection step panicked");
                    }
                }
            }
            stats.duration_ms = start.elapsed().as_millis() as u64;
            stats.log();
        });
    }
}

/// One sweep's work on the store, without the locking and scheduling around it.
#[derive(Clone)]
struct Sweeper {
    /// Store handle for database access.
    store: Store,
    /// How long a tombstone is retained before it may be reclaimed.
    retention_nanos: u64,
    /// Max tombstones deleted per sweep (see [`GC_MAX_DELETIONS_PER_RUN`]).
    max_deletions_per_run: usize,
}

/// The rows of one context a sweep found work on.
#[derive(Debug, Default)]
struct ContextWork {
    /// Expired tombstones, to delete.
    tombstones: Vec<ContextState>,
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
}

impl Sweeper {
    /// Construct with explicit retention/cap. Split out so tests can drive the
    /// sweep with a tiny retention and cap without waiting real time.
    const fn new(store: Store, retention_nanos: u64, max_deletions_per_run: usize) -> Self {
        Self {
            store,
            retention_nanos,
            max_deletions_per_run,
        }
    }

    /// Single pass over the committed `ContextState` keyspace: find every
    /// tombstone whose retention has elapsed, up to `max_deletions_per_run`,
    /// and every other row that lists deleted children.
    ///
    /// One scan covers all contexts (the column is keyed by
    /// `(context_id, state_key)`), so this is O(total state keys) rather than
    /// O(contexts × total state keys). It changes nothing: [`Self::reclaim`]
    /// re-reads every row it acts on.
    ///
    /// `now_nanos` is injected so tests can exercise the wall-clock retention
    /// guard deterministically; production passes the current time.
    fn scan(&self, now_nanos: u64) -> EyreResult<Plan> {
        let mut iter = self.store.iter::<ContextState>()?;
        let mut plan = Plan::default();
        let mut tombstones = 0usize;

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
            // missed this pass — harmless, since a just-created tombstone isn't
            // retention-eligible yet and the next sweep catches it (GC is
            // eventually consistent).
            // Only entity rows can be tombstones or list children, and the key
            // says which rows those are; everything else is skipped without a
            // read.
            let Some(id) = entity_id(&entry) else {
                continue;
            };
            let Some(value) = self.store.get(&entry)? else {
                continue;
            };

            if self.expired(id, value.as_ref(), now_nanos) {
                plan.work
                    .entry(context_id)
                    .or_default()
                    .tombstones
                    .push(entry);
                tombstones += 1;
                if tombstones >= self.max_deletions_per_run {
                    plan.capped = true;
                    break;
                }
            } else if reclaim::lists_deleted_children(id, value.as_ref()) {
                plan.work.entry(context_id).or_default().parents.push(entry);
            }
        }

        Ok(plan)
    }

    /// Whether the row of entity `id` is a tombstone GC may collect at `now`.
    ///
    /// Wall-clock retention. `saturating_sub` keeps a backward clock jump
    /// safe: if `now < deleted_at` the age underflows to 0, so a tombstone is
    /// never reclaimed before its retention has genuinely elapsed — no
    /// premature mass-deletion of still-needed tombstones.
    ///
    /// The comparison is deliberately strict: a tombstone must be OLDER than
    /// the retention period to be reclaimed. At the exact boundary it waits one
    /// more cycle; the difference is a single nanosecond at day-scale
    /// retention. The conservative direction is intentional.
    fn expired(&self, id: Id, value: &[u8], now_nanos: u64) -> bool {
        reclaim::expired_tombstone(id, value, now_nanos, self.retention_nanos)
    }

    /// Reclaim what [`Self::scan`] found in `context_id`: delete the expired
    /// tombstones, then drop every collected child from the deleted children
    /// its parents list. Runs under the context's execution lock.
    ///
    /// Best-effort: a single failed read or write must not abort the step and
    /// strand the rest. Counts are actual changes (work done), not intents;
    /// leftovers retry next cycle. Each change is an independent atomic store
    /// write, so an interrupted step leaves the store consistent: a parent
    /// still listing a collected child is what the store held before this
    /// change, and the sync wire already skips such an entry.
    fn reclaim(&self, context_id: ContextId, work: &ContextWork, now_nanos: u64) -> Reclaimed {
        let mut store = self.store.clone();
        let mut done = Reclaimed::default();

        for key in &work.tombstones {
            // Re-validate against the CURRENT value right before deleting. The
            // scan read this key earlier, and the entity may have been
            // resurrected since (tombstone → live re-add). Deleting only while
            // it is STILL a reclaimable tombstone means GC never removes a
            // resurrected live row.
            let Some(id) = entity_id(key) else {
                continue;
            };
            let still_reclaimable = match self.store.get(key) {
                Ok(Some(value)) => self.expired(id, value.as_ref(), now_nanos),
                Ok(None) => false, // already gone
                Err(e) => {
                    warn!(error = ?e, "GC failed to re-read a tombstone; will retry next cycle");
                    continue;
                }
            };
            if !still_reclaimable {
                continue;
            }

            match store.delete(key) {
                Ok(()) => done.tombstones_collected += 1,
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

    /// A whole sweep without locks, as tests drive it.
    #[cfg(test)]
    fn sweep(&self, now_nanos: u64) -> EyreResult<GCStats> {
        let plan = self.scan(now_nanos)?;
        let mut stats = GCStats {
            contexts_scanned: plan.contexts_scanned,
            capped: plan.capped,
            ..GCStats::default()
        };
        for (context_id, work) in plan.work {
            stats.add(self.reclaim(context_id, &work, now_nanos));
        }
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

/// Current wall-clock time in nanoseconds since the Unix epoch, or `0` if the
/// clock is somehow before the epoch. Returning `0` is the safe direction: it
/// makes every tombstone's age saturate to `0`, so a broken clock skips
/// reclamation rather than deleting live-needed tombstones.
fn now_nanos() -> u64 {
    // `as_nanos()` is `u128`; `try_from` (rather than an `as` truncation that
    // would wrap ~year 2554) falls back to `0`, keeping the broken-clock-skips
    // direction — a wrapped-small `now` would also skip, but `0` is explicit.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(0))
}

impl Actor for GarbageCollector {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            interval_secs = self.interval.as_secs(),
            retention_nanos = self.sweeper.retention_nanos,
            max_deletions_per_run = self.sweeper.max_deletions_per_run,
            "Garbage collection actor started"
        );

        // Sweep once on startup so a node restarted with a backlog of expired
        // tombstones doesn't wait a full interval to reclaim them.
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
                duration_ms = self.duration_ms,
                capped = self.capped,
                "Garbage collection completed"
            );
        }
        if self.capped {
            warn!(
                collected = self.tombstones_collected,
                "GC hit the per-run deletion cap; remaining expired tombstones \
                 will be reclaimed on the next cycle"
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

    const DAY_NANOS: u64 = 86_400_000_000_000;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn gc(store: Store, retention_nanos: u64, cap: usize) -> Sweeper {
        Sweeper::new(store, retention_nanos, cap)
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
    /// a single sweep reclaims the whole tombstoned subtree at once — while a
    /// still-live row (no `deleted_at`) and a within-retention tombstone survive.
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
        // A live row and a freshly-deleted (within-retention) row must survive.
        let live = put_index_row(&store, ctx, [13u8; 32], None);
        let now = deleted_at + 2 * DAY_NANOS;
        let recent = put_index_row(&store, ctx, [14u8; 32], Some(now - DAY_NANOS / 2));

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(now)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 3);
        assert!(!exists(&store, &a));
        assert!(!exists(&store, &b));
        assert!(!exists(&store, &c));
        assert!(exists(&store, &live), "live row must survive");
        assert!(
            exists(&store, &recent),
            "within-retention tombstone must survive"
        );
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

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(deleted_at + 100 * DAY_NANOS)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 1);
        assert!(exists(&store, &written_once));
        assert!(!exists(&store, &editable));
    }

    /// A collected tombstone takes its id in its parent's `deleted_children`
    /// with it, in the same sweep: the id only points at the tombstone. A child
    /// whose tombstone is still within retention stays listed. A child with no
    /// row at all (e.g. a parent that arrived by snapshot, which ships no
    /// tombstones) is inert already, so it goes too. The parent keeps its data
    /// and every other field.
    #[test]
    fn collecting_a_tombstone_drops_it_from_its_parent() {
        let store = store();
        let ctx = ContextId::from([7u8; 32]);
        let deleted_at = 10 * DAY_NANOS;
        let now = deleted_at + 2 * DAY_NANOS;

        let expired = put_index_row(&store, ctx, [70u8; 32], Some(deleted_at));
        let recent = put_index_row(&store, ctx, [71u8; 32], Some(now - DAY_NANOS / 2));
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

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(now)
            .unwrap();

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

        // Nothing left to drop: a second sweep rewrites nothing.
        let again = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
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

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(1000 * DAY_NANOS)
            .unwrap();

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
        let collector = gc(store.clone(), DAY_NANOS, 1);

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

    /// A backward clock jump (now < deleted_at) must never reclaim a tombstone:
    /// `saturating_sub` underflows the age to 0, keeping still-needed tombstones.
    #[test]
    fn backward_clock_skew_never_reclaims() {
        let store = store();
        let ctx = ContextId::from([3u8; 32]);
        let deleted_at = 100 * DAY_NANOS;
        let key = put_index_row(&store, ctx, [30u8; 32], Some(deleted_at));

        // Clock moved back to well before the tombstone was created.
        let now = 50 * DAY_NANOS;
        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(now)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key), "backward skew must not reclaim");
    }

    /// Forward time only reclaims once the retention has genuinely elapsed.
    #[test]
    fn forward_clock_reclaims_only_after_retention() {
        let store = store();
        let ctx = ContextId::from([4u8; 32]);
        let deleted_at = 100 * DAY_NANOS;
        let key = put_index_row(&store, ctx, [40u8; 32], Some(deleted_at));

        // Within retention: not yet reclaimable.
        let within = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(deleted_at + DAY_NANOS / 2)
            .unwrap();
        assert_eq!(within.tombstones_collected, 0);
        assert!(exists(&store, &key));

        // Past retention: reclaimed.
        let past = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(deleted_at + 2 * DAY_NANOS)
            .unwrap();
        assert_eq!(past.tombstones_collected, 1);
        assert!(!exists(&store, &key));
    }

    /// Non-index values (entity data blobs) are never reclaimed, even long past
    /// any retention: they don't pass the `EntityIndex` round-trip guard.
    #[test]
    fn non_index_values_are_never_reclaimed() {
        let store = store();
        let ctx = ContextId::from([5u8; 32]);

        let key = ContextStateKey::new(ctx, entity_key(Id::new([50u8; 32])));
        let mut handle = store.clone();
        handle.put(&key, Slice::from(vec![0xABu8; 128])).unwrap();

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(1000 * DAY_NANOS)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key), "entity data must never be reclaimed");
    }

    /// A row under another kind's key is never reclaimed, even when its bytes
    /// are a valid expired tombstone: only entity keys are candidates.
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

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(1000 * DAY_NANOS)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key));
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
