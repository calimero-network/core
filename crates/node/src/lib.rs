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
use std::time::Duration;

use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use futures_util::StreamExt;
use tracing::error;

mod arbiter_pool;
mod delta_store;
pub mod gc;
pub mod handlers;
mod run;
pub mod services;
mod utils;

pub use run::{start, NodeConfig};
pub use services::{BlobCacheService, DeltaStoreService, TimerManager};

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
    // sync: DELETED - replaced by calimero-sync crate (no actor!)
    pub(crate) network: calimero_network_primitives::client::NetworkClient,
    pub(crate) timers: TimerManager,
    pub(crate) sync_timeout: std::time::Duration,
}

/// Mutable runtime state
#[derive(Clone, Debug)]
pub(crate) struct NodeState {
    pub(crate) blob_cache: BlobCacheService,
    pub(crate) delta_stores: DeltaStoreService,
}

impl NodeState {
    fn new() -> Self {
        Self {
            blob_cache: BlobCacheService::new(),
            delta_stores: DeltaStoreService::new(),
        }
    }

    /// Evict old blobs from cache (delegates to BlobCacheService)
    fn evict_old_blobs(&self) {
        self.blob_cache.evict_old();
    }

    /// Cleanup stale pending deltas (delegates to DeltaStoreService)
    async fn cleanup_stale_deltas(&self, max_age: Duration) -> usize {
        self.delta_stores.cleanup_all_stale(max_age).await
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
        network_client: calimero_network_primitives::client::NetworkClient,
        context_client: ContextClient,
        node_client: NodeClient,
        state: NodeState,
        sync_timeout: std::time::Duration,
    ) -> Self {
        // Create timer manager
        let timer_manager = TimerManager::new(
            state.blob_cache.clone(),
            state.delta_stores.clone(),
            context_client.clone(),
            node_client.clone(),
        );

        Self {
            clients: NodeClients {
                context: context_client,
                node: node_client,
            },
            managers: NodeManagers {
                blobstore,
                network: network_client,
                timers: timer_manager,
                sync_timeout,
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

        // Start all periodic timers (delegates to TimerManager)
        self.managers.timers.start_all_timers(ctx);
    }
}
