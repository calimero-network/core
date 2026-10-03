//! Periodic DAG compaction actor (issue #2026).
//!
//! Every regular delta a context applies is persisted forever in the delta
//! column, so the on-disk DAG log grows linearly with lifetime transaction
//! count. This actor periodically collapses old history: every context holding
//! more than `min_deltas_before_compact` delta rows is pruned to its most-recent
//! `retain_recent_count` deltas, in the durable delta column and, where the
//! context has one, in its in-memory DAG.
//!
//! Pruned history is never needed for convergence — a peer that requests a
//! pruned delta gets "not found" and falls back to HashComparison, which
//! reconciles current state without the delta log. The retained window keeps
//! the cheap incremental delta-catch-up fast-path working for small gaps.
//!
//! Modelled on [`crate::gc::GarbageCollector`]: an interval-scheduled actor.
//! One sweep visits
//!
//! - every context with a live in-memory `DeltaStore`, through
//!   [`DeltaStore::compact`], which prunes the DAG and the rows under the DAG
//!   write lock and the context's execution lock;
//! - every other context the node holds (a cold one: nothing has touched it
//!   since the node started, so it has no `DeltaStore`), by its rows alone,
//!   under the context's execution lock ([`disk::prune_context_rows`]). Its
//!   DAG is never loaded: eligibility is a bounded count of its rows and the
//!   retain window a bounded walk back from its persisted heads.
//!
//! A cold context can be loaded while it is pruned. The execution lock is what
//! makes that safe: every commit of an applied row with the heads holds it, so
//! the window cannot move under the prune, and a `DeltaStore` loading
//! concurrently only reads rows (one deleted under it is skipped, exactly as
//! if it had been pruned before the load).
//!
//! A pruned row takes the rows kept beside it (its events hash and TEE
//! trigger, [`side_rows`]) in the same transaction. Those an earlier
//! compaction left behind are swept after the prunes ([`orphans`]).
//!
//! Deleting rows frees no disk space by itself, so after the deletes each
//! context the sweep reclaimed enough from has its slice of the delta column
//! compacted, and the side tables theirs, with no lock held, by the tombstone
//! GC's bar ([`crate::gc::worth_compacting`]).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use actix::{Actor, AsyncContext, Context, Supervised};
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::DagCompactionConfig;
use calimero_primitives::context::ContextId;
use calimero_store::key;
use calimero_store::Store;
use dashmap::DashMap;
use eyre::Result as EyreResult;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;

pub(crate) mod disk;
pub(crate) mod orphans;
pub(crate) mod side_rows;

use disk::DiskPrune;
use orphans::{OrphanSweep, Swept};

/// Periodic DAG-history compactor.
#[derive(Clone)]
pub struct DagCompactor {
    /// Live per-context in-memory DAGs (shared with the node state). Each
    /// `DeltaStore` is `Arc`-backed, so cloning out of the map shares the
    /// same DAG the apply/sync paths mutate.
    delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
    /// The store and each context's execution lock, for contexts with no
    /// live `DeltaStore`.
    context_client: ContextClient,
    /// Operator-tuned thresholds and cadence.
    config: DagCompactionConfig,
    /// Guards against a sweep overlapping itself: set while a sweep task is
    /// running, so a tick that fires before the previous sweep finishes
    /// (many contexts / slow deletes vs. the interval) is skipped rather
    /// than double-counting metrics and racing DB deletes.
    sweep_in_progress: Arc<AtomicBool>,
    /// Where the orphaned-side-row sweep stands, from one sweep to the next.
    orphans: Arc<Mutex<OrphanSweep>>,
}

impl DagCompactor {
    /// Create a new compactor over the node's live delta stores and every
    /// context in its store.
    pub fn new(
        delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
        context_client: ContextClient,
        config: DagCompactionConfig,
    ) -> Self {
        Self {
            delta_stores,
            context_client,
            config,
            sweep_in_progress: Arc::new(AtomicBool::new(false)),
            orphans: Arc::default(),
        }
    }

    /// Spawn one sweep off the actor mailbox, unless a previous sweep is still
    /// running. Compaction is async (it takes DAG locks), so it must not block
    /// the actor; the shared `Arc`s make the detached task safe — it mutates
    /// the same DAGs as the apply path, serialised by the per-DAG lock.
    fn spawn_sweep(&self) {
        if self
            .sweep_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("Skipping DAG compaction tick: previous sweep still running");
            return;
        }

        let delta_stores = self.delta_stores.clone();
        let context_client = self.context_client.clone();
        let config = self.config;
        let in_progress = self.sweep_in_progress.clone();
        let orphans = self.orphans.clone();
        actix::spawn(async move {
            // Clear the in-progress flag on the way out via RAII, so a panic
            // inside `compact_all` (e.g. a bug in DAG pruning) still releases the
            // guard. A plain post-await `store(false)` would be skipped on unwind,
            // leaving `sweep_in_progress` stuck `true` and silently disabling all
            // future compaction sweeps for the process lifetime.
            let _guard = SweepGuard(in_progress);
            DagCompactor::compact_all(delta_stores, context_client, config, orphans).await;
        });
    }

    /// Compact every context once: the live ones, then the cold ones, then
    /// sweep orphaned side rows, then compact the delta-column slices and the
    /// side tables the deletes are worth compacting. Returns the total delta
    /// rows pruned.
    async fn compact_all(
        delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
        context_client: ContextClient,
        config: DagCompactionConfig,
        orphans: Arc<Mutex<OrphanSweep>>,
    ) -> usize {
        let min = config.min_deltas_before_compact;
        let retain = config.retain_recent_count;
        let store = context_client.datastore().clone();

        // Snapshot (context_id, store) pairs up front: holding a DashMap
        // reference guard across the `.await` below would risk deadlocking
        // against the apply path, which also locks the map. `DeltaStore` is
        // a cheap `Arc` clone.
        let live: Vec<(ContextId, DeltaStore)> = delta_stores
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();

        let contexts_live = live.len();
        let mut reclaimed: BTreeMap<ContextId, DiskPrune> = BTreeMap::new();
        let mut total_pruned = 0;

        for (context_id, delta_store) in live {
            let compaction = delta_store.compact(min, retain).await;
            if compaction.in_memory > 0 {
                debug!(%context_id, pruned = compaction.in_memory, "Compacted context DAG history");
            }
            record(
                &mut reclaimed,
                &mut total_pruned,
                context_id,
                compaction.on_disk,
            );
        }

        // Contexts with no live `DeltaStore`, read without a lock: whether one
        // is over the threshold is re-checked under it before anything goes.
        let cold = {
            let (store, delta_stores) = (store.clone(), delta_stores.clone());
            tokio::task::spawn_blocking(move || cold_candidates(&store, &delta_stores, min)).await
        };
        let cold = match cold {
            Ok(Ok(cold)) => cold,
            Ok(Err(e)) => {
                warn!(?e, "DAG compaction could not list cold contexts");
                Vec::new()
            }
            Err(e) => {
                warn!(?e, "DAG compaction cold-context listing panicked");
                Vec::new()
            }
        };
        let contexts_cold = cold.len();

        for context_id in cold {
            let Some(lock) = context_client.acquire_lock(&context_id).await else {
                debug!(%context_id, "DAG compaction skipped a cold context: no context lock");
                continue;
            };
            // Loaded since it was listed: the live path prunes it next sweep,
            // with what its DAG holds kept.
            if delta_stores.contains_key(&context_id) {
                continue;
            }
            let store = store.clone();
            let pruned = tokio::task::spawn_blocking(move || {
                let _lock = lock;
                disk::prune_context_rows(&store, context_id, min, retain, &HashSet::new())
            })
            .await;
            match pruned {
                Ok(Ok(pruned)) => {
                    if pruned.pruned > 0 {
                        info!(
                            %context_id,
                            rows = pruned.pruned,
                            side_rows = pruned.side_rows,
                            "Compacted cold context DAG history"
                        );
                    }
                    record(&mut reclaimed, &mut total_pruned, context_id, pruned);
                }
                Ok(Err(e)) => {
                    warn!(?e, %context_id, "DAG compaction failed to prune a cold context")
                }
                Err(e) => warn!(?e, %context_id, "DAG compaction of a cold context panicked"),
            }
        }

        let swept = sweep_orphans(&store, &delta_stores, &orphans).await;
        if swept.rows > 0 {
            info!(
                rows = swept.rows,
                bytes = swept.bytes,
                "Swept orphaned delta side rows"
            );
        }
        let side_reclaimed = reclaimed
            .values()
            .map(|prune| prune.side_bytes)
            .sum::<u64>()
            + swept.bytes;

        let compacted = tokio::task::spawn_blocking(move || {
            let mut compacted = 0;
            for (context_id, prune) in reclaimed {
                match disk::compact_pruned_slice(&store, context_id, prune.delta_bytes()) {
                    Ok(true) => compacted += 1,
                    Ok(false) => {}
                    Err(e) => {
                        warn!(?e, %context_id, "DAG compaction failed to compact a delta slice")
                    }
                }
            }
            match disk::compact_side_tables(&store, side_reclaimed) {
                Ok(true) => compacted += 1,
                Ok(false) => {}
                Err(e) => warn!(?e, "DAG compaction failed to compact the delta side tables"),
            }
            compacted
        })
        .await
        .unwrap_or_else(|e| {
            warn!(?e, "DAG compaction of delta slices panicked");
            0
        });

        if total_pruned > 0 || swept.rows > 0 {
            info!(
                contexts_live,
                contexts_cold,
                total_pruned,
                orphans_swept = swept.rows,
                compacted,
                "DAG compaction sweep completed"
            );
        }

        total_pruned
    }
}

/// Count one context's prune into the sweep's totals and metrics.
fn record(
    reclaimed: &mut BTreeMap<ContextId, DiskPrune>,
    total_pruned: &mut usize,
    context_id: ContextId,
    pruned: DiskPrune,
) {
    if pruned.pruned == 0 {
        return;
    }
    crate::node_metrics::observe_compaction_pruned(pruned.pruned);
    *total_pruned += pruned.pruned;
    let _previous = reclaimed.insert(context_id, pruned);
}

/// One step of the orphaned-side-row sweep ([`orphans`]): read the side rows
/// to judge, then what every live DAG holds, then judge them against those
/// and every delta row and absorb record. Blocking steps run off the runtime,
/// and a failure or panic leaves the sweep to start over on the next one.
async fn sweep_orphans(
    store: &Store,
    delta_stores: &DashMap<ContextId, DeltaStore>,
    orphans: &Mutex<OrphanSweep>,
) -> Swept {
    // The sweep never overlaps itself (`sweep_in_progress`), so the state is
    // taken for the duration and put back after; a sweep that fails or
    // panics leaves the default, which only restarts the walk.
    let state = std::mem::take(&mut *orphans.lock().unwrap_or_else(PoisonError::into_inner));

    let read = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || {
            let windows = state.read_windows(&store);
            (state, windows)
        })
        .await
    };
    let (mut state, windows) = match read {
        Ok((state, Ok(windows))) => (state, windows),
        Ok((_, Err(e))) => {
            warn!(?e, "DAG compaction could not read the delta side tables");
            return Swept::default();
        }
        Err(e) => {
            warn!(?e, "DAG compaction side-table read panicked");
            return Swept::default();
        }
    };

    // After the windows: a side row on them was recorded before any of these
    // were read.
    let live: Vec<(ContextId, DeltaStore)> = delta_stores
        .iter()
        .map(|entry| (*entry.key(), entry.value().clone()))
        .collect();
    let mut held = Vec::new();
    for (context_id, delta_store) in live {
        held.extend(
            delta_store
                .held_delta_ids()
                .await
                .into_iter()
                .map(|delta_id| (context_id, delta_id)),
        );
    }

    let settled = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || {
            let swept = state.settle(&store, windows, &held);
            (state, swept)
        })
        .await
    };
    match settled {
        Ok((state, swept)) => {
            *orphans.lock().unwrap_or_else(PoisonError::into_inner) = state;
            swept.unwrap_or_else(|e| {
                warn!(?e, "DAG compaction failed to sweep orphaned side rows");
                Swept::default()
            })
        }
        Err(e) => {
            warn!(?e, "DAG compaction orphaned-side-row sweep panicked");
            Swept::default()
        }
    }
}

/// Contexts in the store with no live `DeltaStore` that hold more than
/// `min_rows` delta rows. Reads each context's row keys up to `min_rows + 1`.
fn cold_candidates(
    store: &Store,
    delta_stores: &DashMap<ContextId, DeltaStore>,
    min_rows: usize,
) -> EyreResult<Vec<ContextId>> {
    let handle = store.handle();
    let mut iter = handle.iter::<key::ContextMeta>()?;
    let mut cold = Vec::new();
    for meta_key in iter.keys() {
        let context_id = meta_key?.context_id();
        if delta_stores.contains_key(&context_id) {
            continue;
        }
        if disk::holds_more_rows_than(store, context_id, min_rows)? {
            cold.push(context_id);
        }
    }
    Ok(cold)
}

/// RAII guard that clears [`DagCompactor::sweep_in_progress`] when the sweep
/// task finishes — whether it returns normally or unwinds on panic — so a
/// failed sweep can never permanently wedge the compactor.
struct SweepGuard(Arc<AtomicBool>);

impl Drop for SweepGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Supervised for DagCompactor {}

impl Actor for DagCompactor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            interval_secs = self.config.check_interval.as_secs(),
            min_deltas_before_compact = self.config.min_deltas_before_compact,
            retain_recent_count = self.config.retain_recent_count,
            "DAG compaction actor started"
        );

        // Sweep once on startup so a node that was already over threshold
        // before (re)start doesn't stay bloated for a full interval.
        self.spawn_sweep();

        let interval = self.config.check_interval;
        let _handle = ctx.run_interval(interval, |act, _ctx| act.spawn_sweep());
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("DAG compaction actor stopped");
    }
}

#[cfg(test)]
mod sweep_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A compaction sweep that unwinds mid-run must still clear
    /// `sweep_in_progress`, or the guard permanently wedges the compactor and no
    /// later sweep ever starts. The flag is released by `SweepGuard::drop`, which
    /// runs on panic, so catching a simulated panic must leave it cleared.
    #[test]
    fn panicking_sweep_clears_in_progress_flag() {
        let flag = Arc::new(AtomicBool::new(false));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Set the flag BEFORE constructing the guard, matching production
            // (`spawn_sweep` sets it via `compare_exchange`, then builds the
            // guard). If it were set after — or not at all — the flag would
            // already be `false` and the final assertion would pass even with a
            // no-op `Drop`; setting it first makes the assertion actually prove
            // the guard cleared it.
            flag.store(true, Ordering::Release);
            let _guard = SweepGuard(Arc::clone(&flag));
            panic!("simulated compaction sweep panic");
        }));
        assert!(result.is_err(), "the sweep must have unwound");

        assert!(
            !flag.load(Ordering::Acquire),
            "SweepGuard must clear the in-progress flag even when the sweep panics"
        );
    }
}
