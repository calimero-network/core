//! Node runtime coordinating WASM execution, state sync, and network communication.
//!
//! Provides `NodeManager` actor and extracted services for managing distributed application state.

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

/// Service clients for context and node operations
#[derive(Debug, Clone)]
pub(crate) struct NodeClients {
    pub(crate) context: ContextClient,
    pub(crate) node: NodeClient,
}

/// Service managers (blobstore, network, timers)
#[derive(Clone, Debug)]
pub(crate) struct NodeManagers {
    pub(crate) blobstore: BlobManager,
    pub(crate) network: calimero_network_primitives::client::NetworkClient,
    pub(crate) timers: TimerManager,
    pub(crate) sync_timeout: std::time::Duration,
}

/// Runtime state (caches for blobs and delta stores)
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

    fn evict_old_blobs(&self) {
        self.blob_cache.evict_old();
    }

    async fn cleanup_stale_deltas(&self, max_age: Duration) -> usize {
        self.delta_stores.cleanup_all_stale(max_age).await
    }
}

/// Main node coordinator managing services and handling sync requests
#[derive(Debug)]
pub struct NodeManager {
    pub(crate) clients: NodeClients,
    pub(crate) managers: NodeManagers,
    pub(crate) state: NodeState,
    pub(crate) sync_rx:
        Option<tokio::sync::mpsc::Receiver<calimero_node_primitives::client::SyncRequest>>,
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

/// Listen for sync requests and process them using DAG catchup strategy
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
                    let _ = tx.send(Ok(
                        calimero_node_primitives::client::SyncResult::NoSyncNeeded,
                    ));
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

/// Execute DAG catchup sync using calimero-sync strategy
async fn perform_sync(
    context_id: &calimero_primitives::context::ContextId,
    peer_id: Option<libp2p::PeerId>,
    context_client: &ContextClient,
    network_client: &calimero_network_primitives::client::NetworkClient,
    delta_stores: &crate::services::DeltaStoreService,
    sync_timeout: std::time::Duration,
) -> eyre::Result<calimero_node_primitives::client::SyncResult> {
    use calimero_node_primitives::client::SyncResult;
    use calimero_sync::strategies::SyncStrategy;
    use futures_util::TryStreamExt;
    use std::pin::pin;

    // Get our owned identity (filtered stream returns only owned)
    let mut members = pin!(context_client.get_context_members(context_id, Some(true)));
    let (our_identity, _) = members
        .try_next()
        .await?
        .ok_or_else(|| eyre::eyre!("No owned identity"))?;

    // Get or create delta store for this context (needs our_identity)
    let delta_store = delta_stores
        .get_or_create_with(context_id, || {
            let context = context_client.get_context(context_id).ok().flatten();
            let root = context
                .as_ref()
                .map(|c| *c.root_hash.as_bytes())
                .unwrap_or([0u8; 32]);

            crate::delta_store::DeltaStore::new(
                root,
                context_client.clone(),
                *context_id,
                our_identity,
            )
        })
        .0;

    // Find peer to sync from
    let target_peer = if let Some(peer) = peer_id {
        peer
    } else {
        // No specific peer - get peers from gossipsub mesh
        // Retry with backoff since mesh takes time to populate after subscribe
        let topic = libp2p::gossipsub::IdentTopic::new(context_id.to_string());
        let mut peers = Vec::new();

        for attempt in 0..10 {
            peers = network_client.mesh_peers(topic.hash()).await;
            if !peers.is_empty() {
                tracing::info!(%context_id, attempt, peer_count = peers.len(), "Found peers in mesh");
                break;
            }

            if attempt < 9 {
                let delay_ms = 200 + (attempt * 50); // Increasing backoff: 200ms, 250ms, 300ms...
                tracing::debug!(%context_id, attempt, delay_ms, "No peers in mesh yet, retrying...");
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }

        if peers.is_empty() {
            tracing::warn!(%context_id, "No peers in mesh after retries - cannot sync");
            return Ok(SyncResult::NoSyncNeeded);
        }

        tracing::info!(%context_id, peer_count = peers.len(), "Found peers in mesh");
        peers[0]
    };

    // Execute DAG catchup
    let strategy = DagCatchup::new(network_client.clone(), context_client.clone(), sync_timeout);
    let sync_result = strategy
        .execute(context_id, &target_peer, &our_identity, &*delta_store)
        .await?;

    // Convert result types
    let result = match sync_result {
        calimero_sync::strategies::SyncResult::NoSyncNeeded => SyncResult::NoSyncNeeded,
        calimero_sync::strategies::SyncResult::DeltaSync { .. } => SyncResult::DeltaSync,
        calimero_sync::strategies::SyncResult::FullResync { .. } => SyncResult::FullResync,
    };

    Ok(result)
}
