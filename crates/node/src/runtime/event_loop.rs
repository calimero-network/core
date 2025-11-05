//! Node Runtime Event Loop - The Heart of the New Architecture
//!
//! This is a simple, clean async event loop that replaces ALL the actor mess.
//! No Actix, no message passing, just plain tokio::select!

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use eyre::Result;
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info};

use crate::delta_store::DeltaStore;
use super::dispatch::{GossipsubMessage, P2pRequest, SyncRequest};

/// Node runtime - clean async orchestration (NO ACTORS!)
pub struct NodeRuntime {
    /// Node client for events and operations
    node_client: NodeClient,
    
    /// Context client for context operations
    context_client: ContextClient,
    
    /// Network client for P2P communication
    network_client: NetworkClient,
    
    /// Sync scheduler (from calimero-sync)
    sync_scheduler: Arc<calimero_sync::SyncScheduler>,
    
    /// Delta stores per context
    delta_stores: Arc<Mutex<HashMap<ContextId, DeltaStore>>>,
    
    /// Sync timeout
    sync_timeout: Duration,
    
    /// Channels for event communication
    gossipsub_rx: mpsc::UnboundedReceiver<GossipsubMessage>,
    p2p_rx: mpsc::UnboundedReceiver<P2pRequest>,
    sync_rx: mpsc::Receiver<SyncRequest>,
}

impl NodeRuntime {
    /// Create a new node runtime
    pub fn new(
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        sync_timeout: Duration,
    ) -> (Self, RuntimeHandles) {
        // Create channels
        let (gossipsub_tx, gossipsub_rx) = mpsc::unbounded_channel();
        let (p2p_tx, p2p_rx) = mpsc::unbounded_channel();
        let (sync_tx, sync_rx) = mpsc::channel(256);
        
        // Create sync scheduler
        let sync_config = calimero_sync::SyncConfig::with_timeout(sync_timeout);
        let sync_scheduler = Arc::new(calimero_sync::SyncScheduler::new(
            node_client.clone(),
            context_client.clone(),
            network_client.clone(),
            sync_config,
        ));
        
        let runtime = Self {
            node_client,
            context_client,
            network_client,
            sync_scheduler,
            delta_stores: Arc::new(Mutex::new(HashMap::new())),
            sync_timeout,
            gossipsub_rx,
            p2p_rx,
            sync_rx,
        };
        
        let handles = RuntimeHandles {
            gossipsub_tx,
            p2p_tx,
            sync_tx,
        };
        
        (runtime, handles)
    }
    
    /// Run the event loop (this is the whole runtime!)
    pub async fn run(mut self) -> Result<()> {
        info!("🚀 Starting new node runtime (NO ACTORS!)");
        
        // Periodic heartbeat
        let mut heartbeat = interval(Duration::from_secs(60));
        
        loop {
            tokio::select! {
                // Handle gossipsub broadcasts (state deltas)
                Some(msg) = self.gossipsub_rx.recv() => {
                    if let Err(e) = self.handle_gossipsub(msg).await {
                        error!(?e, "Failed to handle gossipsub message");
                    }
                }
                
                // Handle P2P requests (delta request, blob, etc)
                Some(req) = self.p2p_rx.recv() => {
                    if let Err(e) = self.handle_p2p(req).await {
                        error!(?e, "Failed to handle P2P request");
                    }
                }
                
                // Handle sync requests (from API or triggers)
                Some(sync_req) = self.sync_rx.recv() => {
                    if let Err(e) = self.handle_sync(sync_req).await {
                        error!(?e, "Failed to handle sync request");
                    }
                }
                
                // Periodic heartbeat (check for contexts needing sync)
                _ = heartbeat.tick() => {
                    debug!("Heartbeat tick");
                    // TODO: Implement periodic sync check
                }
            }
        }
    }
    
    /// Handle gossipsub message (state delta broadcast)
    async fn handle_gossipsub(&mut self, msg: GossipsubMessage) -> Result<()> {
        match msg {
            GossipsubMessage::StateDelta {
                source,
                context_id,
                author_id,
                delta_id,
                parent_ids,
                hlc,
                root_hash,
                artifact,
                nonce,
                events,
            } => {
                info!(%context_id, %author_id, "Handling state delta broadcast");
                
                // Get or create DeltaStore
                let delta_store = {
                    let mut stores = self.delta_stores.lock().await;
                    stores.entry(context_id).or_insert_with(|| {
                        // Get our identity for this context
                        // TODO: Proper identity selection
                        let our_identity = calimero_primitives::identity::PublicKey::from([0; 32]);
                        
                        DeltaStore::new(
                            [0; 32], // root
                            self.context_client.clone(),
                            context_id,
                            our_identity,
                        )
                    }).clone()
                };
                
                // Get our identity
                // TODO: Proper identity selection from context
                let our_identity = calimero_primitives::identity::PublicKey::from([0; 32]);
                
                // Use stateless protocol
                calimero_protocols::gossipsub::state_delta::handle_state_delta(
                    &self.node_client,
                    &self.context_client,
                    &self.network_client,
                    &delta_store,
                    our_identity,
                    self.sync_timeout,
                    source,
                    context_id,
                    author_id,
                    delta_id,
                    parent_ids,
                    hlc,
                    root_hash,
                    artifact,
                    nonce,
                    events,
                )
                .await?;
                
                Ok(())
            }
        }
    }
    
    /// Handle P2P request
    async fn handle_p2p(&mut self, req: P2pRequest) -> Result<()> {
        match req {
            P2pRequest::DeltaRequest {
                mut stream,
                context_id,
                delta_id,
                their_identity,
                our_identity,
            } => {
                info!(%context_id, "Handling delta request");
                
                // Get delta store if it exists
                let delta_store_opt = {
                    let stores = self.delta_stores.lock().await;
                    stores.get(&context_id).cloned()
                };
                
                // Get datastore handle
                let handle = self.context_client.datastore_handle();
                
                // Use stateless protocol
                calimero_protocols::p2p::delta_request::handle_delta_request(
                    &mut stream,
                    context_id,
                    delta_id,
                    their_identity,
                    our_identity,
                    &handle,
                    delta_store_opt.as_ref().map(|s| s as &dyn calimero_protocols::p2p::delta_request::DeltaStore),
                    &self.context_client,
                    self.sync_timeout,
                )
                .await?;
                
                Ok(())
            }
            
            P2pRequest::BlobRequest {
                mut stream,
                context,
                our_identity,
                their_identity,
                blob_id,
            } => {
                info!(context_id=%context.id, "Handling blob request");
                
                // Use stateless protocol
                calimero_protocols::p2p::blob_request::handle_blob_request(
                    &mut stream,
                    &context,
                    our_identity,
                    their_identity,
                    blob_id,
                    &self.node_client,
                    &self.context_client,
                    self.sync_timeout,
                )
                .await?;
                
                Ok(())
            }
            
            P2pRequest::KeyExchange {
                mut stream,
                context,
                our_identity,
                their_identity,
                their_nonce,
            } => {
                info!(context_id=%context.id, "Handling key exchange");
                
                // Use stateless protocol
                calimero_protocols::p2p::key_exchange::handle_key_exchange(
                    &mut stream,
                    &context,
                    our_identity,
                    their_identity,
                    their_nonce,
                    &self.context_client,
                    self.sync_timeout,
                )
                .await?;
                
                Ok(())
            }
        }
    }
    
    /// Handle sync request
    async fn handle_sync(&mut self, req: SyncRequest) -> Result<()> {
        info!(
            context_id=%req.context_id,
            "Handling sync request"
        );
        
        // TODO: Implement sync request handling using calimero-sync
        // This will use the SyncScheduler to orchestrate sync
        
        Ok(())
    }
}

/// Handles for sending messages to the runtime
pub struct RuntimeHandles {
    /// Send gossipsub messages
    pub gossipsub_tx: mpsc::UnboundedSender<GossipsubMessage>,
    
    /// Send P2P requests
    pub p2p_tx: mpsc::UnboundedSender<P2pRequest>,
    
    /// Send sync requests
    pub sync_tx: mpsc::Sender<SyncRequest>,
}

