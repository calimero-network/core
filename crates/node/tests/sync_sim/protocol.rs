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

use calimero_node::sync::{
    HashComparisonConfig, HashComparisonFirstRequest, HashComparisonProtocol, HashComparisonStats,
};
use calimero_node_primitives::sync::{
    InitPayload, StreamMessage, SyncProtocolExecutor, SyncTransport,
};
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result, WrapErr};

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
        // Simulate manager behavior: receive first message for routing
        let first_msg = resp_stream
            .recv()
            .await?
            .ok_or_else(|| eyre::eyre!("Stream closed before first message"))?;

        // Extract first request data (like the manager does)
        let first_request = match first_msg {
            StreamMessage::Init {
                payload:
                    InitPayload::TreeNodeRequest {
                        node_id, max_depth, ..
                    },
                ..
            } => HashComparisonFirstRequest { node_id, max_depth },
            _ => bail!("Expected TreeNodeRequest Init message"),
        };

        // Now call the protocol with the extracted first request
        HashComparisonProtocol::run_responder(
            &mut resp_stream,
            responder_store,
            responder_context,
            identity,
            first_request,
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

    // =========================================================================
    // 3-Node Sync Tests (Issue: reports of 3-node sync failing)
    // =========================================================================

    /// Test 3 nodes where each has unique data, syncing in a chain: A→B→C→A
    ///
    /// This tests the scenario where:
    /// - A has entities 1-3
    /// - B has entities 4-6
    /// - C has entities 7-9
    ///
    /// After chain sync, all should have entities 1-9.
    #[tokio::test]
    async fn test_three_node_chain_sync() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);
        let mut charlie = SimNode::new_in_context("charlie", ctx);

        // Alice has entities 1-3
        for i in 1..=3 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("alice-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Bob has entities 4-6
        for i in 4..=6 {
            bob.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("bob-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Charlie has entities 7-9
        for i in 7..=9 {
            charlie.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("charlie-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Initial state: all different
        assert_ne!(alice.root_hash(), bob.root_hash());
        assert_ne!(bob.root_hash(), charlie.root_hash());
        assert_ne!(alice.root_hash(), charlie.root_hash());

        println!("Initial state:");
        println!(
            "  Alice: {} entities, hash {:?}",
            alice.entity_count(),
            &alice.root_hash()[..4]
        );
        println!(
            "  Bob: {} entities, hash {:?}",
            bob.entity_count(),
            &bob.root_hash()[..4]
        );
        println!(
            "  Charlie: {} entities, hash {:?}",
            charlie.entity_count(),
            &charlie.root_hash()[..4]
        );

        // Step 1: Alice syncs FROM Bob (Alice pulls Bob's data)
        let stats1 = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("alice <- bob sync should succeed");
        println!("\nAfter Alice <- Bob:");
        println!(
            "  Alice: {} entities (transferred {})",
            alice.entity_count(),
            stats1.entities_transferred
        );

        // Step 2: Alice syncs FROM Charlie (Alice pulls Charlie's data)
        let stats2 = execute_hash_comparison_sync(&mut alice, &charlie)
            .await
            .expect("alice <- charlie sync should succeed");
        println!("\nAfter Alice <- Charlie:");
        println!(
            "  Alice: {} entities (transferred {})",
            alice.entity_count(),
            stats2.entities_transferred
        );

        // Now Alice has all 9 entities
        assert_eq!(alice.entity_count(), 9, "Alice should have all 9 entities");

        // Step 3: Bob syncs FROM Alice (Bob pulls Alice's data, which now includes Charlie's)
        let stats3 = execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("bob <- alice sync should succeed");
        println!("\nAfter Bob <- Alice:");
        println!(
            "  Bob: {} entities (transferred {})",
            bob.entity_count(),
            stats3.entities_transferred
        );

        // Step 4: Charlie syncs FROM Alice
        let stats4 = execute_hash_comparison_sync(&mut charlie, &alice)
            .await
            .expect("charlie <- alice sync should succeed");
        println!("\nAfter Charlie <- Alice:");
        println!(
            "  Charlie: {} entities (transferred {})",
            charlie.entity_count(),
            stats4.entities_transferred
        );

        // Final state: ALL should be converged
        println!("\nFinal state:");
        println!(
            "  Alice: {} entities, hash {:?}",
            alice.entity_count(),
            &alice.root_hash()[..4]
        );
        println!(
            "  Bob: {} entities, hash {:?}",
            bob.entity_count(),
            &bob.root_hash()[..4]
        );
        println!(
            "  Charlie: {} entities, hash {:?}",
            charlie.entity_count(),
            &charlie.root_hash()[..4]
        );

        // Verify convergence (Invariant I4)
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "Alice and Bob should converge"
        );
        assert_eq!(
            bob.root_hash(),
            charlie.root_hash(),
            "Bob and Charlie should converge"
        );
        assert_eq!(alice.entity_count(), 9);
        assert_eq!(bob.entity_count(), 9);
        assert_eq!(charlie.entity_count(), 9);
    }

    /// Test 3 nodes with mesh sync (all pairs sync bidirectionally)
    ///
    /// This is a more thorough test: every node syncs with every other node.
    #[tokio::test]
    async fn test_three_node_mesh_sync() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);
        let mut charlie = SimNode::new_in_context("charlie", ctx);

        // Each node has 5 unique entities
        for i in 0..5 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(100 + i),
                format!("a-{i}").into_bytes(),
                EntityMetadata::default(),
            );
            bob.insert_entity_with_metadata(
                EntityId::from_u64(200 + i),
                format!("b-{i}").into_bytes(),
                EntityMetadata::default(),
            );
            charlie.insert_entity_with_metadata(
                EntityId::from_u64(300 + i),
                format!("c-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Full mesh sync: each node pulls from both others
        // Round 1: Everyone pulls from everyone else
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("a<-b");
        execute_hash_comparison_sync(&mut alice, &charlie)
            .await
            .expect("a<-c");
        execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("b<-a");
        execute_hash_comparison_sync(&mut bob, &charlie)
            .await
            .expect("b<-c");
        execute_hash_comparison_sync(&mut charlie, &alice)
            .await
            .expect("c<-a");
        execute_hash_comparison_sync(&mut charlie, &bob)
            .await
            .expect("c<-b");

        // All should have 15 entities and same hash
        assert_eq!(alice.entity_count(), 15, "Alice should have 15 entities");
        assert_eq!(bob.entity_count(), 15, "Bob should have 15 entities");
        assert_eq!(
            charlie.entity_count(),
            15,
            "Charlie should have 15 entities"
        );

        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "Alice and Bob should match"
        );
        assert_eq!(
            bob.root_hash(),
            charlie.root_hash(),
            "Bob and Charlie should match"
        );
    }

    /// Test 3 nodes where one starts empty (fresh join scenario)
    #[tokio::test]
    async fn test_three_node_fresh_join() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);
        let mut charlie = SimNode::new_in_context("charlie", ctx); // Fresh, empty

        // Alice and Bob have shared state
        for i in 1..=5 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("shared-{i}").into_bytes(),
                EntityMetadata::default(),
            );
            bob.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("shared-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Alice and Bob are synced
        assert_eq!(alice.root_hash(), bob.root_hash());
        // Charlie is empty
        assert_eq!(charlie.entity_count(), 0);

        // Charlie joins by syncing from Alice
        execute_hash_comparison_sync(&mut charlie, &alice)
            .await
            .expect("charlie <- alice sync should succeed");

        // Charlie should now match Alice and Bob
        assert_eq!(charlie.root_hash(), alice.root_hash());
        assert_eq!(charlie.entity_count(), 5);
    }

    /// Test 3 nodes with conflicting updates to same entity (CRDT merge)
    #[tokio::test]
    async fn test_three_node_crdt_conflict() {
        use calimero_primitives::crdt::CrdtType;

        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);
        let mut charlie = SimNode::new_in_context("charlie", ctx);

        // All three modify the same entity with different timestamps
        let conflict_id = EntityId::from_u64(999);

        alice.insert_entity_with_metadata(
            conflict_id,
            b"alice-version".to_vec(),
            EntityMetadata::new(CrdtType::lww_register("test"), 100), // oldest
        );
        bob.insert_entity_with_metadata(
            conflict_id,
            b"bob-version".to_vec(),
            EntityMetadata::new(CrdtType::lww_register("test"), 200), // middle
        );
        charlie.insert_entity_with_metadata(
            conflict_id,
            b"charlie-version".to_vec(),
            EntityMetadata::new(CrdtType::lww_register("test"), 300), // newest - should win
        );

        // Sync all to Alice (Alice pulls from both)
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("a<-b");
        execute_hash_comparison_sync(&mut alice, &charlie)
            .await
            .expect("a<-c");

        // Sync others from Alice
        execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("b<-a");
        execute_hash_comparison_sync(&mut charlie, &alice)
            .await
            .expect("c<-a");

        // All should converge to same hash (winner is charlie's version with ts=300)
        assert_eq!(alice.root_hash(), bob.root_hash(), "A and B should match");
        assert_eq!(bob.root_hash(), charlie.root_hash(), "B and C should match");

        // All should have exactly 1 entity
        assert_eq!(alice.entity_count(), 1);
        assert_eq!(bob.entity_count(), 1);
        assert_eq!(charlie.entity_count(), 1);
    }
}
