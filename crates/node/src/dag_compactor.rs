//! Periodic DAG compaction actor (issue #2026).
//!
//! Every regular delta a context applies is persisted forever in the delta
//! column, so the on-disk DAG log grows linearly with lifetime transaction
//! count. This actor periodically collapses old history: for each context
//! whose in-memory DAG has grown past `min_deltas_before_compact`, it prunes
//! everything older than the most-recent `retain_recent_count` deltas from
//! both the in-memory DAG and the durable delta column.
//!
//! Pruned history is never needed for convergence — a peer that requests a
//! pruned delta gets "not found" and falls back to HashComparison, which
//! reconciles current state without the delta log. The retained window keeps
//! the cheap incremental delta-catch-up fast-path working for small gaps.
//!
//! Modelled on [`crate::gc::GarbageCollector`]: an interval-scheduled actor
//! that sweeps every live context. It only visits contexts with a live
//! in-memory `DeltaStore` (those actively applying deltas, i.e. the ones whose
//! logs are growing); cold contexts loaded later compact on a subsequent sweep.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use actix::{Actor, AsyncContext, Context};
use calimero_node_primitives::DagCompactionConfig;
use calimero_primitives::context::ContextId;
use dashmap::DashMap;
use tracing::{debug, info};

use crate::delta_store::DeltaStore;

/// Periodic DAG-history compactor.
#[derive(Clone)]
pub struct DagCompactor {
    /// Live per-context in-memory DAGs (shared with the node state). Each
    /// `DeltaStore` is `Arc`-backed, so cloning out of the map shares the
    /// same DAG the apply/sync paths mutate.
    delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
    /// Operator-tuned thresholds and cadence.
    config: DagCompactionConfig,
    /// Guards against a sweep overlapping itself: set while a sweep task is
    /// running, so a tick that fires before the previous sweep finishes
    /// (many contexts / slow deletes vs. the interval) is skipped rather
    /// than double-counting metrics and racing DB deletes.
    sweep_in_progress: Arc<AtomicBool>,
}

impl DagCompactor {
    /// Create a new compactor over the node's live delta stores.
    pub fn new(
        delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
        config: DagCompactionConfig,
    ) -> Self {
        Self {
            delta_stores,
            config,
            sweep_in_progress: Arc::new(AtomicBool::new(false)),
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
        let config = self.config;
        let in_progress = self.sweep_in_progress.clone();
        actix::spawn(async move {
            // Clear the in-progress flag on the way out via RAII, so a panic
            // inside `compact_all` (e.g. a bug in DAG pruning) still releases the
            // guard. A plain post-await `store(false)` would be skipped on unwind,
            // leaving `sweep_in_progress` stuck `true` and silently disabling all
            // future compaction sweeps for the process lifetime.
            let _guard = SweepGuard(in_progress);
            DagCompactor::compact_all(delta_stores, config).await;
        });
    }

    /// Compact every live context once. Returns the total deltas pruned.
    async fn compact_all(
        delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
        config: DagCompactionConfig,
    ) -> usize {
        // Snapshot (context_id, store) pairs up front: holding a DashMap
        // reference guard across the `.await` below would risk deadlocking
        // against the apply path, which also locks the map. `DeltaStore` is
        // a cheap `Arc` clone.
        let stores: Vec<(ContextId, DeltaStore)> = delta_stores
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();

        let contexts_scanned = stores.len();
        let mut contexts_compacted = 0;
        let mut total_pruned = 0;

        for (context_id, store) in stores {
            let pruned = store
                .compact(config.min_deltas_before_compact, config.retain_recent_count)
                .await;
            if pruned > 0 {
                debug!(%context_id, pruned, "Compacted context DAG history");
                crate::node_metrics::observe_compaction_pruned(pruned);
                contexts_compacted += 1;
                total_pruned += pruned;
            }
        }

        if total_pruned > 0 {
            info!(
                contexts_scanned,
                contexts_compacted, total_pruned, "DAG compaction sweep completed"
            );
        }

        total_pruned
    }
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
