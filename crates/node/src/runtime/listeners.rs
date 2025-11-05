//! Network Listeners - Spawn tasks for network events
//!
//! These spawn background tasks that listen for network events and
//! forward them to the main event loop.

use tokio::sync::mpsc;
use tracing::info;

use super::dispatch::{GossipsubMessage, P2pRequest};

/// Spawn gossipsub listener task
///
/// Listens for broadcast messages (state deltas) and forwards to event loop.
pub fn spawn_gossipsub_listener(
    tx: mpsc::UnboundedSender<GossipsubMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("📡 Gossipsub listener started");
        
        // TODO: Integrate with actual gossipsub network layer
        // For now, this is a placeholder
        //
        // Real implementation will:
        // 1. Subscribe to gossipsub topics
        // 2. Receive broadcast messages
        // 3. Parse BroadcastMessage::StateDelta
        // 4. Forward to event loop via tx.send()
        
        // Keep task alive (will be replaced with actual listener)
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    })
}

/// Spawn P2P listener task
///
/// Listens for P2P request/response streams and forwards to event loop.
pub fn spawn_p2p_listener(
    tx: mpsc::UnboundedSender<P2pRequest>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("🔌 P2P listener started");
        
        // TODO: Integrate with actual P2P network layer
        // For now, this is a placeholder
        //
        // Real implementation will:
        // 1. Listen for incoming P2P streams
        // 2. Parse Init message to determine protocol
        // 3. Create appropriate P2pRequest
        // 4. Forward to event loop via tx.send()
        
        // Keep task alive (will be replaced with actual listener)
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    })
}

