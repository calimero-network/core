//! Garbage collection actor for storage tombstones.
//!
//! This module provides automatic cleanup of old tombstones in the storage layer.
//! Tombstones are created when entities are deleted in the CRDT storage system,
//! and this actor periodically removes tombstones that have exceeded their
//! retention period.

use std::time::Duration;

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_primitives::context::ContextId;
use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
use calimero_storage::index::EntityIndex;
use calimero_store::key::{self, ContextState};
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, error, info, warn};

/// Message to trigger garbage collection.
#[derive(Copy, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct RunGC;

/// Garbage collector actor for removing expired tombstones.
#[derive(Clone, Debug)]
pub struct GarbageCollector {
    /// Store handle for database access
    store: Store,
    /// Interval between automatic GC runs
    interval: Duration,
}

impl GarbageCollector {
    /// Create a new garbage collector.
    ///
    /// # Arguments
    ///
    /// * `store` - Store handle for accessing the database
    /// * `interval` - Time between GC runs (default: 12 hours)
    pub fn new(store: Store, interval: Duration) -> Self {
        Self { store, interval }
    }

    /// Collect garbage across all contexts.
    fn collect_all(&self) -> EyreResult<GCStats> {
        let start = std::time::Instant::now();

        let contexts = self.list_contexts()?;
        let context_count = contexts.len();

        debug!(count = context_count, "Found contexts for GC");

        let mut total_collected = 0;

        for context_id in contexts {
            match self.collect_for_context(&context_id) {
                Ok(count) => {
                    if count > 0 {
                        debug!(
                            context_id = %context_id,
                            tombstones = count,
                            "Collected tombstones for context"
                        );
                    }
                    total_collected += count;
                }
                Err(e) => {
                    warn!(
                        context_id = %context_id,
                        error = ?e,
                        "Failed to collect garbage for context"
                    );
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(GCStats {
            tombstones_collected: total_collected,
            contexts_scanned: context_count,
            duration_ms,
        })
    }

    /// List all contexts in the database.
    fn list_contexts(&self) -> EyreResult<Vec<ContextId>> {
        let mut contexts = Vec::new();

        // Iterate all context metadata entries
        let mut iter = self.store.iter::<key::ContextMeta>()?;

        while let Some(meta) = iter.next()? {
            contexts.push(meta.context_id());
        }

        Ok(contexts)
    }

    /// Collect expired tombstones for a specific context.
    fn collect_for_context(&self, context_id: &ContextId) -> EyreResult<usize> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos() as u64;

        let retention = TOMBSTONE_RETENTION_NANOS;

        // Iterate all state keys for this context
        let mut iter = self.store.iter::<ContextState>()?;

        // We need to collect keys to delete separately to avoid iterator invalidation
        let mut keys_to_delete = Vec::new();

        while let Some(state_entry) = iter.next()? {
            // Only process keys for this context
            if state_entry.context_id() != *context_id {
                continue;
            }

            // Try to parse as storage Index key
            // Index keys are hashed, so we need to read the value to check if it's a tombstone
            if let Some(value) = self.store.get(&state_entry)? {
                // Try to deserialize as EntityIndex
                if let Ok(index) = borsh::from_slice::<EntityIndex>(value.as_ref()) {
                    // Check if it's a tombstone and if it's expired
                    if let Some(deleted_at) = index.deleted_at {
                        let age = now.saturating_sub(deleted_at);

                        if age > retention {
                            keys_to_delete.push(state_entry);
                        }
                    }
                }
            }
        }

        // Delete expired tombstones
        let collected = keys_to_delete.len();

        let mut store = self.store.clone();
        for key in keys_to_delete {
            store.delete(&key)?;
        }

        Ok(collected)
    }
}

impl Actor for GarbageCollector {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            interval_secs = self.interval.as_secs(),
            "Garbage collection actor started"
        );

        // Schedule periodic GC runs
        let interval = self.interval;
        let _handle = ctx.run_interval(interval, |_act, ctx| {
            ctx.notify(RunGC);
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("Garbage collection actor stopped");
    }
}

impl Handler<RunGC> for GarbageCollector {
    type Result = ();

    fn handle(&mut self, _msg: RunGC, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Starting garbage collection cycle");

        match self.collect_all() {
            Ok(stats) => {
                info!(
                    tombstones_collected = stats.tombstones_collected,
                    contexts_scanned = stats.contexts_scanned,
                    duration_ms = stats.duration_ms,
                    "Garbage collection completed"
                );
            }
            Err(e) => {
                error!(error = ?e, "Garbage collection failed");
            }
        }
    }
}

/// Statistics from a garbage collection run.
#[derive(Debug)]
struct GCStats {
    /// Number of tombstones collected
    tombstones_collected: usize,
    /// Number of contexts scanned
    contexts_scanned: usize,
    /// Duration of the GC run in milliseconds
    duration_ms: u64,
}
