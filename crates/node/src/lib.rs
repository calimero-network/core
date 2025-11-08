//! Calimero node orchestration and coordination.
//!
//! **Purpose**: Main node runtime that coordinates sync, storage, networking, and event handling.
//! **Key Components**:
//! - `NodeManager`: Main actor coordinating all services
//! - `NodeClients`: External service clients (context, node)
//! - `NodeManagers`: Service managers (blobstore, sync)
//! - `NodeState`: Runtime state (caches)

#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use dashmap::DashMap;
use futures_util::StreamExt;
use tracing::{debug, error, warn};

use crate::delta_store::DeltaStore;

mod arbiter_pool;
mod delta_store;
pub mod gc;
pub mod handlers;
mod run;
pub mod sync;
mod utils;

pub use run::{start, NodeConfig};
pub use sync::SyncManager;

/// External service clients (injected dependencies)
#[derive(Debug, Clone)]
pub(crate) struct NodeClients {
    pub(crate) context: ContextClient,
    pub(crate) node: NodeClient,
}

/// Service managers (injected dependencies)
#[derive(Clone, Debug)]
pub(crate) struct NodeManagers {
    pub(crate) blobstore: BlobManager,
    pub(crate) sync: SyncManager,
}

/// Mutable runtime state
#[derive(Clone, Debug)]
pub(crate) struct NodeState {
    pub(crate) delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
}

impl NodeState {
    fn new() -> Self {
        Self {
            delta_stores: Arc::new(DashMap::new()),
        }
    }
}

/// Main node orchestrator.
///
/// **SRP Applied**: Clear separation of:
/// - `clients`: External service clients (context, node)
/// - `managers`: Service managers (blobstore, sync)
/// - `state`: Mutable runtime state (caches)
#[derive(Debug)]
pub struct NodeManager {
    pub(crate) clients: NodeClients,
    pub(crate) managers: NodeManagers,
    pub(crate) state: NodeState,
}

impl NodeManager {
    pub(crate) fn new(
        blobstore: BlobManager,
        sync_manager: SyncManager,
        context_client: ContextClient,
        node_client: NodeClient,
        state: NodeState,
    ) -> Self {
        Self {
            clients: NodeClients {
                context: context_client,
                node: node_client,
            },
            managers: NodeManagers {
                blobstore,
                sync: sync_manager,
            },
            state,
        }
    }
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let node_client = self.clients.node.clone();
        let contexts = self.clients.context.get_context_ids(None);

        // Subscribe to all contexts
        let _handle = ctx.spawn(
            async move {
                let mut contexts = pin!(contexts);

                while let Some(context_id) = contexts.next().await {
                    let Ok(context_id) = context_id else {
                        error!("Failed to get context ID");
                        continue;
                    };

                    if let Err(err) = node_client.subscribe(&context_id).await {
                        error!(%context_id, %err, "Failed to subscribe to context");
                    }
                }
            }
            .into_actor(self),
        );

        // Periodic cleanup of stale pending deltas (every 60 seconds)
        let _handle = ctx.run_interval(Duration::from_secs(60), |act, ctx| {
            let max_age = Duration::from_secs(300); // 5 minutes timeout
            let delta_stores = act.state.delta_stores.clone();

            let _ignored = ctx.spawn(
                async move {
                    for entry in delta_stores.iter() {
                        let context_id = *entry.key();
                        let delta_store = entry.value();

                        // Evict stale deltas
                        let evicted = delta_store.cleanup_stale(max_age).await;

                        if evicted > 0 {
                            warn!(
                                %context_id,
                                evicted_count = evicted,
                                "Evicted stale pending deltas (timed out after 5 min)"
                            );
                        }

                        // Log stats for monitoring
                        let stats = delta_store.pending_stats().await;
                        if stats.count > 0 {
                            debug!(
                                %context_id,
                                pending_count = stats.count,
                                oldest_age_secs = stats.oldest_age_secs,
                                missing_parents = stats.total_missing_parents,
                                "Pending delta statistics"
                            );

                            // Trigger snapshot fallback if too many pending
                            const SNAPSHOT_THRESHOLD: usize = 100;
                            if stats.count > SNAPSHOT_THRESHOLD {
                                warn!(
                                    %context_id,
                                    pending_count = stats.count,
                                    threshold = SNAPSHOT_THRESHOLD,
                                    "Too many pending deltas - state sync will recover on next periodic sync"
                                );
                            }
                        }
                    }
                }
                .into_actor(act),
            );
        });

        // Periodic hash heartbeat broadcast (every 30 seconds)
        // Allows peers to detect silent divergence
        let _handle = ctx.run_interval(Duration::from_secs(30), |act, ctx| {
            let context_client = act.clients.context.clone();
            let node_client = act.clients.node.clone();

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
                .into_actor(act),
            );
        });
    }
}
