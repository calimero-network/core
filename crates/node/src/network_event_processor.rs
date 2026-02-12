//! Network event processor bridge.
//!
//! This module bridges the dedicated network event channel to the NodeManager actor.
//! It runs as a tokio task that receives events from the channel and forwards them
//! to the NodeManager via Actix messages.
//!
//! ## Why This Exists
//!
//! The previous architecture used `LazyRecipient<NetworkEvent>` directly in NetworkManager
//! to send events across Actix arbiters. Under high load, messages were silently lost.
//!
//! This bridge:
//! 1. Receives events from a dedicated mpsc channel (guaranteed delivery or explicit drop)
//! 2. Forwards them to NodeManager via Actix's `do_send` (which is reliable within-arbiter)
//! 3. Provides visibility into channel pressure via metrics
//!
//! The actual event processing still happens in NodeManager, preserving the existing
//! async spawn patterns that work within Actix's actor model.

use std::sync::Arc;

use actix::Addr;
use calimero_network_primitives::messages::NetworkEvent;
use tokio::sync::Notify;
use tracing::{debug, info};

use crate::network_event_channel::NetworkEventReceiver;
use crate::NodeManager;

/// Bridge that forwards events from the channel to NodeManager.
///
/// This ensures events are reliably delivered to the NodeManager actor,
/// avoiding the cross-arbiter message loss issues.
pub struct NetworkEventBridge {
    /// Channel receiver for incoming events.
    receiver: NetworkEventReceiver,

    /// NodeManager actor address.
    node_manager: Addr<NodeManager>,

    /// Shutdown signal.
    shutdown: Arc<Notify>,
}

impl NetworkEventBridge {
    /// Create a new bridge.
    pub fn new(receiver: NetworkEventReceiver, node_manager: Addr<NodeManager>) -> Self {
        Self {
            receiver,
            node_manager,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Get a shutdown handle to signal graceful shutdown.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run the bridge loop.
    ///
    /// This should be spawned as a tokio task. It will run until:
    /// - The channel is closed (sender dropped)
    /// - Shutdown is signaled via the notify handle
    pub async fn run(mut self) {
        info!("Network event bridge started");

        loop {
            tokio::select! {
                // Process next event
                event = self.receiver.recv() => {
                    match event {
                        Some(event) => {
                            self.forward_event(event);
                        }
                        None => {
                            info!("Network event channel closed, shutting down bridge");
                            break;
                        }
                    }
                }

                // Shutdown signal
                _ = self.shutdown.notified() => {
                    info!("Network event bridge received shutdown signal");
                    break;
                }
            }
        }

        // Graceful shutdown: drain remaining events
        self.graceful_shutdown();

        info!("Network event bridge stopped");
    }

    /// Forward a single event to NodeManager.
    fn forward_event(&self, event: NetworkEvent) {
        // Log event type for debugging
        let event_type = match &event {
            NetworkEvent::Message { .. } => "Message",
            NetworkEvent::StreamOpened { .. } => "StreamOpened",
            NetworkEvent::Subscribed { .. } => "Subscribed",
            NetworkEvent::Unsubscribed { .. } => "Unsubscribed",
            NetworkEvent::ListeningOn { .. } => "ListeningOn",
            NetworkEvent::BlobRequested { .. } => "BlobRequested",
            NetworkEvent::BlobProvidersFound { .. } => "BlobProvidersFound",
            NetworkEvent::BlobDownloaded { .. } => "BlobDownloaded",
            NetworkEvent::BlobDownloadFailed { .. } => "BlobDownloadFailed",
            NetworkEvent::SpecializedNodeVerificationRequest { .. } => {
                "SpecializedNodeVerificationRequest"
            }
            NetworkEvent::SpecializedNodeInvitationResponse { .. } => {
                "SpecializedNodeInvitationResponse"
            }
        };

        debug!(event_type, "Forwarding network event to NodeManager");

        // Forward to NodeManager - this uses Actix's do_send which is reliable
        // within the same Actix system
        self.node_manager.do_send(event);
    }

    /// Graceful shutdown: drain and forward remaining events.
    fn graceful_shutdown(&mut self) {
        info!("Draining remaining network events...");

        let remaining_events = self.receiver.drain();
        let count = remaining_events.len();

        if count > 0 {
            info!(count, "Forwarding remaining events before shutdown");

            for event in remaining_events {
                self.forward_event(event);
            }
        }

        info!("Graceful shutdown complete");
    }
}

// Re-export the old name for backwards compatibility during transition
pub type NetworkEventProcessor = NetworkEventBridge;

// Re-export config (not really needed anymore but kept for API compatibility)
/// Configuration for the network event processor (bridge).
#[derive(Debug, Clone, Default)]
pub struct NetworkEventProcessorConfig {
    /// Unused - kept for API compatibility
    pub sync_timeout: std::time::Duration,
}

impl From<&crate::sync::SyncConfig> for NetworkEventProcessorConfig {
    fn from(sync_config: &crate::sync::SyncConfig) -> Self {
        Self {
            sync_timeout: sync_config.timeout,
        }
    }
}
