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

// For sync implementation
use calimero_sync::strategies::dag_catchup::DagCatchup;

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
    pub(crate) sync_rx: Option<tokio::sync::mpsc::Receiver<calimero_node_primitives::client::SyncRequest>>,
}

impl NodeManager {
    pub(crate) fn new(
        blobstore: BlobManager,
        network_client: calimero_network_primitives::client::NetworkClient,
        context_client: ContextClient,
        node_client: NodeClient,
        state: NodeState,
        sync_timeout: std::time::Duration,
        sync_rx: tokio::sync::mpsc::Receiver<calimero_node_primitives::client::SyncRequest>,
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
            sync_rx: Some(sync_rx),
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

        // Start sync request listener
        if let Some(sync_rx) = self.sync_rx.take() {
            start_sync_listener(
                ctx,
                sync_rx,
                self.clients.context.clone(),
                self.managers.network.clone(),
                self.state.delta_stores.clone(),
                self.managers.sync_timeout,
            );
        }
    }
}

/// Start listening for sync requests
///
/// Spawns an async task that processes sync requests from the channel
fn start_sync_listener(
    _ctx: &mut actix::Context<NodeManager>,
    mut sync_rx: tokio::sync::mpsc::Receiver<calimero_node_primitives::client::SyncRequest>,
    context_client: ContextClient,
    network_client: calimero_network_primitives::client::NetworkClient,
    delta_stores: crate::services::DeltaStoreService,
    sync_timeout: std::time::Duration,
) {
    use tracing::{error, info};

    info!("Starting sync request listener");

    // Spawn sync listener as background task (local task since it's not Send)
    actix::spawn(async move {
        while let Some((context_id, peer_id, result_tx)) = sync_rx.recv().await {
            let Some(ctx_id) = context_id else {
                // Global sync not implemented
                if let Some(tx) = result_tx {
                    let _ = tx.send(Ok(calimero_node_primitives::client::SyncResult::NoSyncNeeded));
                }
                continue;
            };

            info!(%ctx_id, ?peer_id, "Processing sync request");

            // Perform sync
            let result = perform_sync(
                &ctx_id,
                peer_id,
                &context_client,
                &network_client,
                &delta_stores,
                sync_timeout,
            )
            .await;

            // Send result back
            if let Some(tx) = result_tx {
                match &result {
                    Ok(sync_result) => {
                        info!(%ctx_id, ?sync_result, "Sync completed successfully");
                        let _ = tx.send(Ok(*sync_result));
                    }
                    Err(e) => {
                        error!(%ctx_id, error = %e, "Sync failed");
                        let _ = tx.send(Err(eyre::eyre!("Sync failed: {}", e)));
                    }
                }
            }
        }
    });
}

/// Perform DAG catchup sync for a context
///
/// Extracted as a free function to avoid lifetime issues in async blocks
async fn perform_sync(
    context_id: &calimero_primitives::context::ContextId,
    peer_id: Option<libp2p::PeerId>,
    context_client: &ContextClient,
    network_client: &calimero_network_primitives::client::NetworkClient,
    delta_stores: &crate::services::DeltaStoreService,
    sync_timeout: std::time::Duration,
) -> eyre::Result<calimero_node_primitives::client::SyncResult> {
    use calimero_node_primitives::client::SyncResult;
    use tracing::{info, warn};

    // Get delta store
    let delta_store = delta_stores
        .get(context_id)
        .ok_or_else(|| eyre::eyre!("Context {} not found in delta stores", context_id))?;

    // Get our identity (filter for owned=true, take first)
    let our_identity = {
        use futures_util::TryStreamExt;
        use std::pin::pin;
        
        let mut members = pin!(context_client.get_context_members(context_id, Some(true)));
        
        // get_context_members(Some(true)) filters for owned identities
        // Just take the first one (there should only be one owned identity per node)
        let (public_key, _is_owned) = members
            .try_next()
            .await?
            .ok_or_else(|| eyre::eyre!("No owned identity found for context {}", context_id))?;
        
        public_key
    };

    // Find peer
    let target_peer = if let Some(peer) = peer_id {
        peer
    } else {
        warn!(
            %context_id,
            "No peer specified for sync - returning NoSyncNeeded"
        );
        return Ok(SyncResult::NoSyncNeeded);
    };

    info!(%context_id, %target_peer, %our_identity, "Starting DAG catchup");

    // Use calimero-sync DagCatchup strategy
    let strategy = DagCatchup::new(
        network_client.clone(),
        context_client.clone(),
        sync_timeout,
    );

    // Execute sync (DeltaStore implements the trait from protocols)
    use calimero_sync::strategies::SyncStrategy;
    let sync_result = strategy
        .execute(context_id, &target_peer, &our_identity, &*delta_store)
        .await?;

    // Convert calimero_sync::SyncResult to calimero_node_primitives::SyncResult
    let result = match sync_result {
        calimero_sync::strategies::SyncResult::NoSyncNeeded => SyncResult::NoSyncNeeded,
        calimero_sync::strategies::SyncResult::DeltaSync { .. } => SyncResult::DeltaSync,
        calimero_sync::strategies::SyncResult::FullResync { .. } => SyncResult::FullResync,
    };

    info!(%context_id, ?result, "Sync completed");

    Ok(result)
}
