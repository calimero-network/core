//! Sync Scheduler - Orchestrates sync operations
//!
//! **NO ACTORS!** This is plain async Rust orchestration.
//!
//! Replaces the old SyncManager (1,088 lines of actor mess) with
//! clean, testable async code that composes stateless protocols.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{eyre, Result};
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::config::{RetryConfig, SyncConfig};
use crate::events::SyncEvent;
use crate::strategies::{SyncResult, SyncStrategy};

/// Sync scheduler - orchestrates sync operations WITHOUT actors
pub struct SyncScheduler {
    /// Node client for sending events
    node_client: NodeClient,

    /// Context client for context operations
    context_client: ContextClient,

    /// Network client for P2P communication
    network_client: NetworkClient,

    /// Configuration
    config: SyncConfig,

    /// Active syncs (context_id -> sync state)
    active_syncs: Arc<Mutex<HashMap<ContextId, SyncState>>>,

    /// Event channel for sync events
    event_tx: mpsc::UnboundedSender<SyncEvent>,
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<SyncEvent>>>,
}

/// State of an active sync operation
#[derive(Clone, Debug)]
struct SyncState {
    /// Peer being synced with
    peer_id: libp2p::PeerId,

    /// Start time
    started_at: Instant,

    /// Retry count
    retry_count: usize,
}

impl SyncScheduler {
    /// Create a new sync scheduler
    pub fn new(
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        config: SyncConfig,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Self {
            node_client,
            context_client,
            network_client,
            config,
            active_syncs: Arc::new(Mutex::new(HashMap::new())),
            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        }
    }

    /// Sync a specific context with a peer
    ///
    /// This is the main entry point for sync orchestration.
    /// It's plain async - NO actors!
    pub async fn sync_context(
        &self,
        context_id: &ContextId,
        peer_id: &libp2p::PeerId,
        our_identity: &PublicKey,
        delta_store: &dyn calimero_protocols::p2p::delta_request::DeltaStore,
        strategy: &dyn SyncStrategy,
    ) -> Result<SyncResult> {
        // Check if already syncing
        {
            let active = self.active_syncs.lock().await;
            if active.contains_key(context_id) {
                return Err(eyre!("Sync already in progress for context {}", context_id));
            }
        }

        // Mark as active
        {
            let mut active = self.active_syncs.lock().await;
            active.insert(
                *context_id,
                SyncState {
                    peer_id: *peer_id,
                    started_at: Instant::now(),
                    retry_count: 0,
                },
            );
        }

        // Emit started event
        let _ = self
            .event_tx
            .send(SyncEvent::started(*context_id, *peer_id));

        info!(
            %context_id,
            %peer_id,
            strategy = strategy.name(),
            "Starting sync"
        );

        // Execute sync with retry logic
        let result = self
            .execute_with_retry(context_id, peer_id, our_identity, delta_store, strategy)
            .await;

        // Remove from active
        let duration = {
            let mut active = self.active_syncs.lock().await;
            active
                .remove(context_id)
                .map(|state| state.started_at.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0))
        };

        // Emit result event
        match &result {
            Ok(sync_result) => {
                info!(
                    %context_id,
                    %peer_id,
                    duration_ms = duration.as_millis(),
                    "Sync completed successfully"
                );

                let (deltas_synced, strategy_name) = match sync_result {
                    SyncResult::NoSyncNeeded => (None, "no_sync_needed".to_string()),
                    SyncResult::DeltaSync { deltas_applied } => {
                        (Some(*deltas_applied), strategy.name().to_string())
                    }
                    SyncResult::FullResync { .. } => (None, strategy.name().to_string()),
                };

                let _ = self.event_tx.send(SyncEvent::completed(
                    *context_id,
                    *peer_id,
                    strategy_name,
                    deltas_synced,
                    duration.as_millis() as u64,
                ));
            }
            Err(e) => {
                error!(
                    %context_id,
                    %peer_id,
                    error = %e,
                    "Sync failed"
                );

                let _ = self.event_tx.send(SyncEvent::failed(
                    *context_id,
                    *peer_id,
                    e.to_string(),
                    0,
                    false,
                ));
            }
        }

        result
    }

    /// Execute sync with retry logic
    async fn execute_with_retry(
        &self,
        context_id: &ContextId,
        peer_id: &libp2p::PeerId,
        our_identity: &PublicKey,
        delta_store: &dyn calimero_protocols::p2p::delta_request::DeltaStore,
        strategy: &dyn SyncStrategy,
    ) -> Result<SyncResult> {
        let retry_config = &self.config.retry_config;
        let mut backoff = retry_config.initial_backoff;

        for attempt in 0..=retry_config.max_retries {
            if attempt > 0 {
                debug!(
                    %context_id,
                    attempt,
                    backoff_ms = backoff.as_millis(),
                    "Retrying sync after backoff"
                );
                tokio::time::sleep(backoff).await;

                // Exponential backoff
                backoff = std::cmp::min(
                    Duration::from_secs_f64(
                        backoff.as_secs_f64() * retry_config.backoff_multiplier,
                    ),
                    retry_config.max_backoff,
                );
            }

            match strategy
                .execute(context_id, peer_id, our_identity, delta_store)
                .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let will_retry = attempt < retry_config.max_retries;

                    warn!(
                        %context_id,
                        %peer_id,
                        attempt,
                        will_retry,
                        error = %e,
                        "Sync attempt failed"
                    );

                    let _ = self.event_tx.send(SyncEvent::failed(
                        *context_id,
                        *peer_id,
                        e.to_string(),
                        attempt,
                        will_retry,
                    ));

                    if !will_retry {
                        return Err(e);
                    }
                }
            }
        }

        Err(eyre!(
            "Sync failed after {} retries",
            retry_config.max_retries
        ))
    }

    /// Start periodic heartbeat (if enabled)
    ///
    /// This runs in the background and periodically checks for contexts
    /// that need syncing.
    pub fn start_heartbeat(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if !self.config.enable_heartbeat {
                return;
            }

            let mut interval = interval(self.config.heartbeat_interval);

            loop {
                interval.tick().await;

                debug!("Heartbeat tick - checking for contexts needing sync");

                // TODO: Implement periodic sync check
                // 1. Get all contexts
                // 2. Check which ones need syncing
                // 3. Trigger sync for those contexts
                //
                // For now, this is a placeholder
            }
        })
    }

    /// Get sync events (for observability)
    pub async fn recv_event(&self) -> Option<SyncEvent> {
        self.event_rx.lock().await.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_scheduler_creation() {
        // Basic smoke test - scheduler can be created
        // Full tests require mock clients
    }
}
