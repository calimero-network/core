//! Sync strategies
//!
//! Different strategies for synchronizing state between nodes.

pub mod dag_catchup;
pub mod state_resync;

use async_trait::async_trait;
use calimero_primitives::context::ContextId;
use eyre::Result;

/// Result of a sync operation
#[derive(Clone, Debug)]
pub enum SyncResult {
    /// No sync needed - already in sync
    NoSyncNeeded,
    
    /// Delta sync performed (DAG catchup)
    DeltaSync {
        /// Number of deltas applied
        deltas_applied: usize,
    },
    
    /// Full state resync performed
    FullResync {
        /// New root hash after resync
        root_hash: [u8; 32],
    },
}

/// Sync strategy trait
///
/// Each strategy implements a different approach to synchronization:
/// - `DagCatchup`: Fetch missing deltas (most common)
/// - `StateResync`: Full state resync (fallback for large gaps)
///
/// **Design**: Strategies are stateless - all dependencies passed as parameters.
/// This makes them testable and composable.
#[async_trait(?Send)]
pub trait SyncStrategy: Send + Sync {
    /// Execute the sync strategy
    ///
    /// # Arguments
    /// * `context_id` - Context to sync
    /// * `peer_id` - Peer to sync with
    /// * `our_identity` - Our identity in this context
    /// * `delta_store` - DeltaStore for this context (injected!)
    ///
    /// # Returns
    /// Result of the sync operation
    async fn execute(
        &self,
        context_id: &ContextId,
        peer_id: &libp2p::PeerId,
        our_identity: &calimero_primitives::identity::PublicKey,
        delta_store: &dyn calimero_protocols::p2p::delta_request::DeltaStore,
    ) -> Result<SyncResult>;
    
    /// Get strategy name (for logging/metrics)
    fn name(&self) -> &'static str;
}

