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
        }
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

        let context_count = stores.len();
        let mut total_pruned = 0;

        for (context_id, store) in stores {
            let pruned = store
                .compact(config.min_deltas_before_compact, config.retain_recent_count)
                .await;
            if pruned > 0 {
                debug!(%context_id, pruned, "Compacted context DAG history");
                crate::node_metrics::observe_compaction_pruned(pruned);
                total_pruned += pruned;
            }
        }

        if total_pruned > 0 {
            info!(
                contexts_scanned = context_count,
                total_pruned, "DAG compaction sweep completed"
            );
        }

        total_pruned
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

        let interval = self.config.check_interval;
        let _handle = ctx.run_interval(interval, |act, _ctx| {
            // Compaction is async (it takes DAG locks); run it off the
            // actor mailbox so a long sweep never blocks the actor. The
            // shared `Arc`s make this safe — the spawned task mutates the
            // same DAGs as the apply path, serialised by the per-DAG lock.
            let delta_stores = act.delta_stores.clone();
            let config = act.config;
            actix::spawn(async move {
                let _ = DagCompactor::compact_all(delta_stores, config).await;
            });
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("DAG compaction actor stopped");
    }
}
