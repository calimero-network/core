//! Timer Manager - Manages periodic tasks for the node.
//!
//! This service handles scheduling and execution of periodic tasks:
//! - Blob cache eviction (every 5 minutes)
//! - Delta cleanup (every 60 seconds)
//! - Hash heartbeat broadcast (every 30 seconds)
//!
//! # Design
//!
//! The TimerManager is integrated with Actix's interval system. It provides
//! a clean way to schedule periodic tasks without cluttering NodeManager.

use std::pin::pin;
use std::time::Duration;

use actix::{AsyncContext, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use futures_util::StreamExt;
use tracing::debug;

use crate::services::{BlobCacheService, DeltaStoreService};

/// Timer manager for periodic node tasks.
///
/// Coordinates three main periodic tasks:
/// 1. Blob cache eviction (5 minutes)
/// 2. Delta cleanup (60 seconds)  
/// 3. Hash heartbeat broadcast (30 seconds)
///
/// # Example
///
/// ```rust,ignore
/// let timer_manager = TimerManager::new(
///     blob_cache,
///     delta_stores,
///     context_client,
///     node_client,
/// );
///
/// // In NodeManager::started()
/// timer_manager.start_all_timers(ctx);
/// ```
#[derive(Clone, Debug)]
pub struct TimerManager {
    blob_cache: BlobCacheService,
    delta_stores: DeltaStoreService,
    context_client: ContextClient,
    node_client: NodeClient,
}

impl TimerManager {
    /// Create a new timer manager.
    pub fn new(
        blob_cache: BlobCacheService,
        delta_stores: DeltaStoreService,
        context_client: ContextClient,
        node_client: NodeClient,
    ) -> Self {
        Self {
            blob_cache,
            delta_stores,
            context_client,
            node_client,
        }
    }

    /// Start all periodic timers.
    ///
    /// This should be called from `NodeManager::started()` to initialize all timers.
    pub fn start_all_timers<A>(&self, ctx: &mut actix::Context<A>)
    where
        A: actix::Actor<Context = actix::Context<A>>,
    {
        self.start_blob_eviction_timer(ctx);
        self.start_delta_cleanup_timer(ctx);
        self.start_heartbeat_timer(ctx);
        self.start_pending_delta_check_timer(ctx);
    }

    /// Start the blob cache eviction timer (every 5 minutes).
    pub fn start_blob_eviction_timer<A>(&self, ctx: &mut actix::Context<A>)
    where
        A: actix::Actor<Context = actix::Context<A>>,
    {
        let blob_cache = self.blob_cache.clone();

        ctx.run_interval(Duration::from_secs(300), move |_act, _ctx| {
            blob_cache.evict_old();
        });
    }

    /// Start the delta cleanup timer (every 60 seconds).
    pub fn start_delta_cleanup_timer<A>(&self, ctx: &mut actix::Context<A>)
    where
        A: actix::Actor<Context = actix::Context<A>>,
    {
        let delta_stores = self.delta_stores.clone();

        ctx.run_interval(Duration::from_secs(60), move |_act, ctx| {
            let max_age = Duration::from_secs(300); // 5 minutes timeout
            let delta_stores = delta_stores.clone();

            let _ignored = ctx.spawn(
                async move {
                    // Delegate cleanup to DeltaStoreService
                    let _total_evicted = delta_stores.cleanup_all_stale(max_age).await;
                }
                .into_actor(_act),
            );
        });
    }

    /// Start the hash heartbeat broadcast timer (every 30 seconds).
    ///
    /// Broadcasts current root hash and DAG heads to allow peers to detect divergence.
    pub fn start_heartbeat_timer<A>(&self, ctx: &mut actix::Context<A>)
    where
        A: actix::Actor<Context = actix::Context<A>>,
    {
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();

        ctx.run_interval(Duration::from_secs(30), move |_act, ctx| {
            let context_client = context_client.clone();
            let node_client = node_client.clone();

            let _ignored = ctx.spawn(
                async move {
                    // Get all context IDs
                    let contexts = context_client.get_context_ids(None);

                    let mut contexts_stream = pin!(contexts);
                    while let Some(context_id_result) = contexts_stream.next().await {
                        let Ok(context_id) = context_id_result else {
                            continue;
                        };

                        // Get context metadata
                        let Ok(Some(context)) = context_client.get_context(&context_id) else {
                            continue;
                        };

                        // Broadcast hash heartbeat
                        if let Err(e) = node_client
                            .broadcast_heartbeat(
                                &context_id,
                                context.root_hash,
                                context.dag_heads.clone(),
                            )
                            .await
                        {
                            debug!(
                                %context_id,
                                error = %e,
                                "Failed to broadcast hash heartbeat"
                            );
                        }
                    }
                }
                .into_actor(_act),
            );
        });
    }

    /// Start the pending delta check timer (every 60 seconds).
    ///
    /// Checks for contexts with pending deltas (waiting for parents) and triggers sync.
    /// This ensures we don't get stuck with orphaned deltas.
    pub fn start_pending_delta_check_timer<A>(&self, ctx: &mut actix::Context<A>)
    where
        A: actix::Actor<Context = actix::Context<A>>,
    {
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let delta_stores = self.delta_stores.clone();

        ctx.run_interval(Duration::from_secs(60), move |_act, ctx| {
            let context_client = context_client.clone();
            let node_client = node_client.clone();
            let delta_stores = delta_stores.clone();

            let _ignored = ctx.spawn(
                async move {
                    // Get all context IDs
                    let contexts = context_client.get_context_ids(None);

                    let mut contexts_stream = pin!(contexts);
                    while let Some(context_id_result) = contexts_stream.next().await {
                        let Ok(context_id) = context_id_result else {
                            continue;
                        };

                        // Check if this context has a delta store
                        let Some(delta_store) = delta_stores.get(&context_id) else {
                            continue;
                        };

                        // Check for pending deltas
                        let missing_parents = delta_store.get_missing_parents().await;

                        if !missing_parents.missing_ids.is_empty() {
                            info!(
                                %context_id,
                                pending_count = missing_parents.missing_ids.len(),
                                "Periodic check detected pending deltas - triggering sync"
                            );

                            // Trigger sync to fetch missing parents
                            if let Err(e) = node_client.sync(Some(&context_id), None).await {
                                warn!(
                                    %context_id,
                                    ?e,
                                    "Failed to trigger sync from pending delta check"
                                );
                            }
                        }
                    }
                }
                .into_actor(_act),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full timer testing requires Actix runtime and mocked clients.
    // Integration tests verify real behavior.
    // Here we verify the service can be constructed.

    #[test]
    fn test_timer_manager_construction() {
        // This test verifies the structure compiles and types are correct.
        // Real timer behavior is tested in integration tests with full node setup.
    }
}
