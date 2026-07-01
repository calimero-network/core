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

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
use calimero_storage::index::EntityIndex;
use calimero_store::key::ContextState;
use calimero_store::layer::{ReadLayer, WriteLayer};
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
    /// Store handle for database access.
    store: Store,
    /// Interval between automatic GC runs.
    interval: Duration,
    /// How long a tombstone is retained before it may be reclaimed.
    retention_nanos: u64,
    /// Max tombstones deleted per sweep (see [`GC_MAX_DELETIONS_PER_RUN`]).
    max_deletions_per_run: usize,
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
    /// * `interval` - Time between GC runs (default: 12 hours)
    pub fn new(store: Store, interval: Duration) -> Self {
        Self::with_limits(
            store,
            interval,
            TOMBSTONE_RETENTION_NANOS,
            GC_MAX_DELETIONS_PER_RUN,
        )
    }

    /// Construct with explicit retention/cap. Split out so tests can drive the
    /// sweep with a tiny retention and cap without waiting real time.
    fn with_limits(
        store: Store,
        interval: Duration,
        retention_nanos: u64,
        max_deletions_per_run: usize,
    ) -> Self {
        Self {
            store,
            interval,
            retention_nanos,
            max_deletions_per_run,
            sweep_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn one sweep off the actor mailbox unless a previous sweep is still
    /// running. The store scan is synchronous RocksDB work, so it runs on a
    /// blocking thread (`spawn_blocking`) and never stalls the actor's reactor.
    fn spawn_sweep(&self) {
        if self
            .sweep_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("Skipping GC tick: previous sweep still running");
            return;
        }

        let this = self.clone();
        let in_progress = self.sweep_in_progress.clone();
        let _handle = actix::spawn(async move {
            let outcome = tokio::task::spawn_blocking(move || this.sweep(now_nanos())).await;

            match outcome {
                Ok(Ok(stats)) => {
                    if stats.tombstones_collected > 0 || stats.capped {
                        info!(
                            tombstones_collected = stats.tombstones_collected,
                            contexts_scanned = stats.contexts_scanned,
                            duration_ms = stats.duration_ms,
                            capped = stats.capped,
                            "Garbage collection completed"
                        );
                    }
                }
                Ok(Err(e)) => error!(error = ?e, "Garbage collection failed"),
                Err(join_err) => error!(error = ?join_err, "Garbage collection task panicked"),
            }

            in_progress.store(false, Ordering::Release);
        });
    }

    /// Single pass over the committed `ContextState` keyspace: delete every
    /// tombstone whose retention has elapsed, up to `max_deletions_per_run`.
    ///
    /// One scan covers all contexts (the column is keyed by
    /// `(context_id, state_key)`), so this is O(total state keys) rather than
    /// O(contexts × total state keys).
    ///
    /// `now_nanos` is injected so tests can exercise the wall-clock retention
    /// guard deterministically; production passes the current time.
    fn sweep(&self, now_nanos: u64) -> EyreResult<GCStats> {
        let start = Instant::now();

        // Iterate once; collect the keys to delete separately so we don't mutate
        // the column while its iterator is live.
        let mut iter = self.store.iter::<ContextState>()?;
        let mut keys_to_delete = Vec::new();
        let mut contexts = HashSet::new();
        let mut capped = false;

        while let Some(entry) = iter.next()? {
            let _ = contexts.insert(entry.context_id());

            let Some(value) = self.store.get(&entry)? else {
                continue;
            };
            let Some(deleted_at) = tombstone_deleted_at(value.as_ref()) else {
                continue;
            };

            // Wall-clock retention. `saturating_sub` keeps a backward clock jump
            // safe: if `now < deleted_at` the age underflows to 0, so a tombstone
            // is never reclaimed before its retention has genuinely elapsed — no
            // premature mass-deletion of still-needed tombstones.
            let age = now_nanos.saturating_sub(deleted_at);
            if age <= self.retention_nanos {
                continue;
            }

            keys_to_delete.push(entry);
            if keys_to_delete.len() >= self.max_deletions_per_run {
                capped = true;
                break;
            }
        }

        // Delete the expired tombstones. Each delete is an independent atomic
        // store write, so an interrupted sweep leaves the store consistent: the
        // tombstones already deleted stay deleted, the rest remain valid
        // tombstones and are reclaimed on the next pass.
        let collected = keys_to_delete.len();
        let mut store = self.store.clone();
        for key in keys_to_delete {
            store.delete(&key)?;
        }

        if capped {
            warn!(
                collected,
                "GC hit the per-run deletion cap; remaining expired tombstones \
                 will be reclaimed on the next cycle"
            );
        }

        Ok(GCStats {
            tombstones_collected: collected,
            contexts_scanned: contexts.len(),
            duration_ms: start.elapsed().as_millis() as u64,
            capped,
        })
    }
}

/// Reads the tombstone deletion time from a raw `ContextState` value, or `None`
/// if the value is not a tombstoned index row.
///
/// `ContextState` keys are hashed (the `Index`/`Entry` type tag lives inside the
/// pre-image and can't be recovered from the key), so an index row can only be
/// told apart from entity data by its value shape. A value qualifies only if it
/// deserializes as an `EntityIndex`, carries a `deleted_at`, AND re-serializes to
/// the exact same bytes: borsh is canonical, so a coincidental partial decode of
/// application data fails the round-trip. This guarantees GC never deletes live
/// entity data that merely happens to borsh-decode.
fn tombstone_deleted_at(value: &[u8]) -> Option<u64> {
    let index = borsh::from_slice::<EntityIndex>(value).ok()?;
    // Cheap check first: only round-trip values that are actually tombstones.
    let deleted_at = index.deleted_at?;
    let reserialized = borsh::to_vec(&index).ok()?;
    (reserialized == value).then_some(deleted_at)
}

/// Current wall-clock time in nanoseconds since the Unix epoch, or `0` if the
/// clock is somehow before the epoch. Returning `0` is the safe direction: it
/// makes every tombstone's age saturate to `0`, so a broken clock skips
/// reclamation rather than deleting live-needed tombstones.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

impl Actor for GarbageCollector {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            interval_secs = self.interval.as_secs(),
            retention_nanos = self.retention_nanos,
            max_deletions_per_run = self.max_deletions_per_run,
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

/// Statistics from a garbage collection run.
#[derive(Debug)]
struct GCStats {
    /// Number of tombstones collected.
    tombstones_collected: usize,
    /// Number of distinct contexts observed during the sweep.
    contexts_scanned: usize,
    /// Duration of the GC run in milliseconds.
    duration_ms: u64,
    /// Whether the sweep stopped early at the per-run deletion cap.
    capped: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_primitives::context::ContextId;
    use calimero_storage::address::Id;
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

    fn gc(store: Store, retention_nanos: u64, cap: usize) -> GarbageCollector {
        GarbageCollector::with_limits(store, Duration::from_secs(3600), retention_nanos, cap)
    }

    /// Writes a `ContextState` row whose value is a borsh-serialized
    /// `EntityIndex` carrying `deleted_at`. `state_key` distinguishes rows in the
    /// column. Returns the key so tests can assert on its presence.
    fn put_index_row(
        store: &Store,
        ctx: ContextId,
        state_key: [u8; 32],
        deleted_at: Option<u64>,
    ) -> ContextStateKey {
        let mut index = EntityIndex::minimal_for_test(Id::new(state_key));
        index.deleted_at = deleted_at;
        let bytes = borsh::to_vec(&index).unwrap();

        let key = ContextStateKey::new(ctx, state_key);
        let mut handle = store.clone();
        handle.put(&key, Slice::from(bytes)).unwrap();
        key
    }

    fn exists(store: &Store, key: &ContextStateKey) -> bool {
        store.get(key).unwrap().is_some()
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

        let key = ContextStateKey::new(ctx, [50u8; 32]);
        let mut handle = store.clone();
        handle.put(&key, Slice::from(vec![0xABu8; 128])).unwrap();

        let stats = gc(store.clone(), DAY_NANOS, GC_MAX_DELETIONS_PER_RUN)
            .sweep(1000 * DAY_NANOS)
            .unwrap();

        assert_eq!(stats.tombstones_collected, 0);
        assert!(exists(&store, &key), "entity data must never be reclaimed");
    }
}
