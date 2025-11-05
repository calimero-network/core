//! DAG Catchup Strategy
//!
//! Fetches missing deltas from peers to fill gaps in the DAG.
//! This is the most common sync strategy.

use async_trait::async_trait;
use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{eyre, Result};
use tracing::{debug, info};

use super::{SyncResult, SyncStrategy};

/// DAG Catchup strategy - fetch missing deltas
///
/// **Stateless**: All dependencies injected at execution time.
pub struct DagCatchup {
    /// Network client for P2P communication
    network_client: NetworkClient,

    /// Context client for context operations
    context_client: ContextClient,

    /// Sync timeout
    timeout: std::time::Duration,
}

impl DagCatchup {
    /// Create a new DAG catchup strategy
    pub fn new(
        network_client: NetworkClient,
        context_client: ContextClient,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            network_client,
            context_client,
            timeout,
        }
    }
}

#[async_trait(?Send)]
impl SyncStrategy for DagCatchup {
    async fn execute(
        &self,
        context_id: &ContextId,
        peer_id: &libp2p::PeerId,
        our_identity: &PublicKey,
        delta_store: &dyn calimero_protocols::p2p::delta_request::DeltaStore,
    ) -> Result<SyncResult> {
        info!(
            %context_id,
            %peer_id,
            "Executing DAG catchup strategy"
        );

        // Get missing parent IDs from DeltaStore
        let mut missing_result = delta_store.get_missing_parents().await;

        // If no missing parents, check if DAG is empty (initial sync case)
        if missing_result.missing_ids.is_empty() {
            // Check if we have ANY deltas at all
            let has_genesis = delta_store.dag_has_delta_applied(&[0; 32]).await;
            
            if !has_genesis {
                // Empty DAG - need initial sync!
                // Request peer's DAG heads to bootstrap
                debug!(%context_id, "Empty DAG detected - requesting peer's heads for initial sync");
                
                let peer_heads = calimero_protocols::p2p::delta_request::request_dag_heads(
                    &self.network_client,
                    *context_id,
                    *peer_id,
                    *our_identity,
                    &self.context_client,
                    self.timeout,
                )
                .await?;
                
                if peer_heads.is_empty() {
                    debug!(%context_id, "Peer has no deltas either - both empty");
                    return Ok(SyncResult::NoSyncNeeded);
                }
                
                info!(%context_id, heads_count = peer_heads.len(), "Got peer heads for initial sync");
                missing_result.missing_ids = peer_heads;
            } else {
                debug!(%context_id, "No missing deltas - sync not needed");
                return Ok(SyncResult::NoSyncNeeded);
            }
        }

        info!(
            %context_id,
            missing_count = missing_result.missing_ids.len(),
            "Requesting missing deltas from peer"
        );

        // Use the stateless protocol to request missing deltas
        calimero_protocols::p2p::delta_request::request_missing_deltas(
            &self.network_client,
            *context_id,
            missing_result.missing_ids.clone(),
            *peer_id,
            delta_store,
            *our_identity,
            &self.context_client,
            self.timeout,
        )
        .await?;

        info!(
            %context_id,
            deltas_synced = missing_result.missing_ids.len(),
            "DAG catchup completed successfully"
        );

        Ok(SyncResult::DeltaSync {
            deltas_applied: missing_result.missing_ids.len(),
        })
    }

    fn name(&self) -> &'static str {
        "dag_catchup"
    }
}
