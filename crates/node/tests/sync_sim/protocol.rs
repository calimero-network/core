//! Protocol execution for simulation testing.
//!
//! Runs the **production** sync protocol implementations using simulation
//! infrastructure (`SimStream`, `SimStorage`) for end-to-end testing.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    execute_hash_comparison_sync                  │
//! │                                                                  │
//! │  ┌────────────────────┐         ┌────────────────────┐         │
//! │  │  Initiator Task    │         │  Responder Task    │         │
//! │  │  (alice)           │◄───────►│  (bob)             │         │
//! │  │                    │ SimStream│                    │         │
//! │  │  Store (InMemory)  │  pair   │  Store (InMemory)  │         │
//! │  └────────────────────┘         └────────────────────┘         │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Design: Same Code, Different Backends
//!
//! This module calls the **exact same** `HashComparisonProtocol` that runs
//! in production. The only difference is the backends:
//! - Production: `StreamTransport` (network) + `Store<RocksDB>`
//! - Simulation: `SimStream` (channels) + `Store<InMemoryDB>`
//!
//! # Invariants Tested
//!
//! - **I4**: Strategy equivalence (same final state as other protocols)
//! - **I5**: No silent data loss (CRDT merge at leaves)
//! - **I6**: Delta buffering during sync

use calimero_node::sync::{HashComparisonConfig, HashComparisonProtocol, HashComparisonStats};
use calimero_node_primitives::sync::SyncProtocolExecutor;
use calimero_primitives::identity::PublicKey;
use eyre::{Result, WrapErr};

use super::node::SimNode;
use super::transport::SimStream;

/// Statistics from a simulated HashComparison sync session.
///
/// This is a thin wrapper around the production `HashComparisonStats`
/// with simulation-specific additions if needed.
#[derive(Debug, Default, Clone)]
pub struct SimSyncStats {
    /// Number of tree nodes compared.
    pub nodes_compared: u64,
    /// Number of leaf entities transferred.
    pub entities_transferred: u64,
    /// Number of nodes skipped (hashes matched).
    pub nodes_skipped: u64,
    /// Number of request/response rounds.
    pub rounds: u64,
}

impl From<HashComparisonStats> for SimSyncStats {
    fn from(stats: HashComparisonStats) -> Self {
        Self {
            nodes_compared: stats.nodes_compared,
            entities_transferred: stats.entities_merged,
            nodes_skipped: stats.nodes_skipped,
            rounds: stats.requests_sent,
        }
    }
}

/// Execute HashComparison sync between two SimNodes.
///
/// This runs the **production** `HashComparisonProtocol`:
/// 1. Creates bidirectional SimStream
/// 2. Spawns initiator and responder tasks
/// 3. Returns when sync completes
///
/// # Arguments
///
/// * `initiator` - Node initiating sync (will pull from responder)
/// * `responder` - Node responding to sync requests
///
/// # Returns
///
/// Statistics about the sync session.
///
/// # Example
///
/// ```ignore
/// let mut alice = SimNode::new("alice");
/// let mut bob = SimNode::new("bob");
///
/// // Set up state...
///
/// let stats = execute_hash_comparison_sync(&mut alice, &mut bob).await?;
/// assert_eq!(alice.root_hash(), bob.root_hash()); // Converged!
/// ```
pub async fn execute_hash_comparison_sync(
    initiator: &mut SimNode,
    responder: &SimNode,
) -> Result<SimSyncStats> {
    let (mut init_stream, mut resp_stream) = SimStream::pair();

    // Get root hashes for comparison
    let init_root = initiator.root_hash();
    let resp_root = responder.root_hash();

    // If already in sync, no work needed
    if init_root == resp_root {
        return Ok(SimSyncStats::default());
    }

    // Get stores and context info
    let initiator_store = initiator.storage().store();
    let responder_store = responder.storage().store();
    let initiator_context = initiator.context_id();
    let responder_context = responder.context_id();

    // Dummy identity for simulation
    let identity = PublicKey::from([0u8; 32]);

    // Config for initiator
    let config = HashComparisonConfig {
        remote_root_hash: resp_root,
    };

    // Run both sides concurrently using the PRODUCTION protocol
    let initiator_fut = async {
        HashComparisonProtocol::run_initiator(
            &mut init_stream,
            initiator_store,
            initiator_context,
            identity,
            config,
        )
        .await
    };

    let responder_fut = async {
        HashComparisonProtocol::run_responder(
            &mut resp_stream,
            responder_store,
            responder_context,
            identity,
        )
        .await
    };

    // Run both sides
    let (init_result, resp_result) = tokio::join!(initiator_fut, responder_fut);

    // Check for errors
    resp_result.wrap_err("responder failed")?;
    let stats = init_result.wrap_err("initiator failed")?;

    Ok(stats.into())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_sim::actions::EntityMetadata;
    use crate::sync_sim::types::EntityId;
    use calimero_primitives::context::ContextId;

    /// Create a shared context ID for testing.
    fn shared_context() -> ContextId {
        ContextId::from(SimNode::DEFAULT_CONTEXT_ID)
    }

    #[tokio::test]
    async fn test_sync_empty_to_populated() {
        // Both nodes share the same context (as they would in production)
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Bob has entities, Alice is empty
        bob.insert_entity_with_metadata(
            EntityId::from_u64(1),
            b"hello".to_vec(),
            EntityMetadata::default(),
        );
        bob.insert_entity_with_metadata(
            EntityId::from_u64(2),
            b"world".to_vec(),
            EntityMetadata::default(),
        );

        assert_ne!(alice.root_hash(), bob.root_hash());

        // Sync
        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        // Verify convergence (Invariant I4)
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "root hashes should match after sync"
        );
        assert!(
            stats.entities_transferred > 0,
            "should have transferred entities"
        );
        assert_eq!(
            alice.entity_count(),
            bob.entity_count(),
            "entity counts should match after sync"
        );
    }

    #[tokio::test]
    async fn test_sync_already_in_sync() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let bob = SimNode::new_in_context("bob", ctx);

        // Both empty = already in sync
        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        assert_eq!(stats.rounds, 0, "no rounds needed when already in sync");
    }

    #[tokio::test]
    async fn test_sync_partial_overlap() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared entity
        let shared_id = EntityId::from_u64(100);
        alice.insert_entity_with_metadata(shared_id, b"shared".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(shared_id, b"shared".to_vec(), EntityMetadata::default());

        // Bob-only entity
        bob.insert_entity_with_metadata(
            EntityId::from_u64(200),
            b"bob-only".to_vec(),
            EntityMetadata::default(),
        );

        // Sync
        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        // Verify Alice got Bob's entity
        assert!(stats.entities_transferred >= 1);
    }
}
