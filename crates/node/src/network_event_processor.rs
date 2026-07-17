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
use std::time::Duration;

use actix::Addr;
use calimero_network_primitives::messages::NetworkEvent;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::network_event_channel::{NetworkEventChannelMetrics, NetworkEventReceiver};
use crate::NodeManager;

/// Bound on NodeManager's actor mailbox. `try_send` (below) respects this, so
/// the network-event feed can no longer grow the mailbox without limit — unlike
/// `do_send`, which ignores capacity entirely.
pub(crate) const NODE_MANAGER_MAILBOX_CAPACITY: usize = 1024;

/// How many times `forward_event` retries a full mailbox before dropping.
const MAILBOX_SEND_MAX_RETRIES: usize = 20;

/// Delay between mailbox-full retries. `MAILBOX_SEND_MAX_RETRIES` × this bounds
/// how long a single event is held (and how long the source channel is left to
/// backpressure) before the event is dropped.
const MAILBOX_SEND_RETRY_BACKOFF: Duration = Duration::from_millis(25);

/// Stable label for a network event, for logs and drop metrics.
fn event_type_str(event: &NetworkEvent) -> &'static str {
    match event {
        NetworkEvent::Message { .. } => "Message",
        NetworkEvent::StreamOpened { .. } => "StreamOpened",
        NetworkEvent::Subscribed { .. } => "Subscribed",
        NetworkEvent::Unsubscribed { .. } => "Unsubscribed",
        NetworkEvent::ListeningOn { .. } => "ListeningOn",
        NetworkEvent::BlobRequested { .. } => "BlobRequested",
        NetworkEvent::BlobProvidersFound { .. } => "BlobProvidersFound",
        NetworkEvent::BlobDownloaded { .. } => "BlobDownloaded",
        NetworkEvent::BlobDownloadFailed { .. } => "BlobDownloadFailed",
    }
}

/// Bridge that forwards events from the channel to NodeManager.
///
/// This ensures events are reliably delivered to the NodeManager actor,
/// avoiding the cross-arbiter message loss issues.
pub struct NetworkEventBridge {
    /// Channel receiver for incoming events.
    receiver: NetworkEventReceiver,

    /// NodeManager actor address.
    node_manager: Addr<NodeManager>,

    /// Shared channel metrics, so a mailbox-full drop is counted in the same
    /// `events_dropped` series as source-channel drops.
    metrics: NetworkEventChannelMetrics,

    /// Shutdown signal.
    shutdown: Arc<Notify>,
}

impl NetworkEventBridge {
    /// Create a new bridge.
    pub fn new(receiver: NetworkEventReceiver, node_manager: Addr<NodeManager>) -> Self {
        Self {
            metrics: receiver.metrics(),
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
                            self.forward_event(event).await;
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
    ///
    /// Uses `try_send` against NodeManager's bounded mailbox instead of
    /// `do_send`: `do_send` ignores the mailbox capacity, so draining the
    /// bounded source channel straight into it let the mailbox grow without
    /// limit. A short bounded retry rides out transient fullness (and, by not
    /// pulling the next event, lets the source channel apply its own
    /// backpressure); a mailbox that stays full means NodeManager is
    /// overwhelmed, so the event is dropped with a counter rather than buffered
    /// forever.
    async fn forward_event(&self, event: NetworkEvent) {
        let event_type = event_type_str(&event);
        debug!(event_type, "Forwarding network event to NodeManager");

        let mut event = event;
        for _ in 0..MAILBOX_SEND_MAX_RETRIES {
            match self.node_manager.try_send(event) {
                Ok(()) => return,
                Err(actix::dev::SendError::Closed(_)) => {
                    debug!(event_type, "NodeManager mailbox closed; dropping event");
                    return;
                }
                Err(actix::dev::SendError::Full(returned)) => {
                    event = returned;
                    tokio::time::sleep(MAILBOX_SEND_RETRY_BACKOFF).await;
                }
            }
        }

        // Sustained-full mailbox: drop with visibility rather than grow it.
        self.metrics.events_dropped.inc();
        warn!(
            event_type,
            capacity = NODE_MANAGER_MAILBOX_CAPACITY,
            "NodeManager mailbox full after retries; dropping network event"
        );
    }

    /// Graceful shutdown: drain and forward remaining events.
    fn graceful_shutdown(&mut self) {
        info!("Draining remaining network events...");

        let remaining_events = self.receiver.drain();
        let count = remaining_events.len();

        if count > 0 {
            info!(count, "Forwarding remaining events before shutdown");

            // Best-effort flush on the way down: the mailbox-bounding concern
            // does not apply during shutdown, so use `do_send` to hand every
            // remaining event over without dropping any.
            for event in remaining_events {
                self.node_manager.do_send(event);
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
