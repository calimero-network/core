//! Protocol execution for simulation testing.
//!
//! Runs the **production** sync protocol implementations using simulation
//! infrastructure (`SimStream`, `SimStorage`) for end-to-end testing.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    execute_hash_comparison_sync                 │
//! │                                                                 │
//! │  ┌────────────────────┐          ┌────────────────────┐         │
//! │  │  Initiator Task    │          │  Responder Task    │         │
//! │  │  (alice)           │◄───────-►│  (bob)             │         │
//! │  │                    │ SimStream│                    │         │
//! │  │  Store (InMemory)  │  pair   │  Store (InMemory)   │         │
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
    /// Number of leaf entities transferred (pulled from peer).
    pub entities_transferred: u64,
    /// Number of leaf entities pushed to peer (bidirectional sync).
    pub entities_pushed: u64,
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
            entities_pushed: stats.entities_pushed,
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

    // =========================================================================
    // Bidirectional Sync Tests (Bug: initiator-has-more-data)
    // =========================================================================
    // These tests reproduce the bug from FIX-HASH-COMPARISON-SYNC.md where
    // HashComparison protocol fails to transfer data when the INITIATOR has
    // more data than the RESPONDER. The old protocol was pull-only.
    //
    // NOTE: Empty nodes (root_hash == [0;32]) use Snapshot, not HashComparison.
    // The bug only manifests when BOTH nodes have state but the initiator has
    // MORE entities. The root hashes differ but neither is zero.

    /// **BUG REPRODUCTION**: Both nodes have state but initiator has more.
    ///
    /// Mirrors the CI failure scenario:
    /// - Both nodes share a base entity (both initialized, has_state=true)
    /// - Node 1 (alice) then writes 10 additional seed entities
    /// - Alice initiates sync with Bob via HashComparison
    /// - Old behavior: alice pulls nothing (bob has no new data), alice's
    ///   local-only subtrees are ignored → bob never gets the seed data
    /// - Fixed behavior: alice detects local-only children and pushes them
    #[tokio::test]
    async fn test_initiator_has_more_data_push_to_peer() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Both share a base entity (so both have has_state=true, non-zero root)
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        // Sanity: both have the same base state
        assert_eq!(alice.root_hash(), bob.root_hash());

        // Now alice writes 10 additional seed entities
        for i in 1..=10 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("seed-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        assert_eq!(alice.entity_count(), 11); // 1 base + 10 seed
        assert_eq!(bob.entity_count(), 1); // 1 base only
        assert_ne!(alice.root_hash(), bob.root_hash());

        // Alice initiates sync WITH bob (alice is initiator = puller)
        // With the bug: alice gets nothing from bob, bob gets nothing.
        // With the fix: alice pushes her local-only data to bob.
        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        println!(
            "Stats: entities_pushed={}, entities_transferred={}, rounds={}",
            stats.entities_pushed, stats.entities_transferred, stats.rounds
        );

        // Bob should now have all 11 entities (1 base + 10 pushed by alice)
        assert_eq!(
            bob.entity_count(),
            11,
            "Bob should have all 11 entities after bidirectional sync"
        );

        // Alice should still have her 11 entities
        assert_eq!(alice.entity_count(), 11);

        // Root hashes should converge
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "Root hashes should match after sync"
        );

        // Verify stats: entities were pushed, not pulled
        assert!(
            stats.entities_pushed >= 10,
            "Should have pushed at least 10 entities, got {}",
            stats.entities_pushed
        );
    }

    /// **BUG REPRODUCTION**: Both nodes have unique data, initiator has MORE.
    ///
    /// Alice has 1 shared + 10 unique, Bob has 1 shared + 3 unique.
    /// After single sync, both should have 1 + 10 + 3 = 14.
    #[tokio::test]
    async fn test_bidirectional_both_have_unique_data() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base entity
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());

        // Alice adds 10 unique entities
        for i in 1..=10 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("alice-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // Bob adds 3 unique entities (completely different IDs)
        for i in 101..=103 {
            bob.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("bob-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        assert_eq!(alice.entity_count(), 11); // 1 shared + 10 unique
        assert_eq!(bob.entity_count(), 4); // 1 shared + 3 unique
        assert_ne!(alice.root_hash(), bob.root_hash());

        // Alice initiates sync WITH bob
        // - Alice should pull Bob's 3 unique entities (entities_transferred)
        // - Alice should push her 10 unique entities to Bob (entities_pushed)
        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        println!(
            "Stats: pushed={}, transferred={}, compared={}, skipped={}, rounds={}",
            stats.entities_pushed,
            stats.entities_transferred,
            stats.nodes_compared,
            stats.nodes_skipped,
            stats.rounds
        );

        // Alice should have all 14 entities (1 shared + 10 own + 3 from bob)
        assert_eq!(
            alice.entity_count(),
            14,
            "Alice should have 14 entities (1 shared + 10 own + 3 from bob)"
        );

        // Bob should also have all 14 entities (1 shared + 3 own + 10 from alice)
        assert_eq!(
            bob.entity_count(),
            14,
            "Bob should have 14 entities (1 shared + 3 own + 10 from alice)"
        );

        // Root hashes should converge
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "Root hashes should match after bidirectional sync"
        );
    }

    /// Verify that single-direction pull sync still works (no regression).
    ///
    /// Both have base state, Bob has extra data, Alice initiates → Alice pulls.
    #[tokio::test]
    async fn test_pull_direction_still_works() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());

        // Bob has 5 extra entities
        for i in 1..=5 {
            bob.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("bob-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        assert_eq!(alice.entity_count(), 6, "Alice should have 6 entities");
        assert_eq!(alice.root_hash(), bob.root_hash(), "Hashes should match");
        assert!(
            stats.entities_transferred >= 5,
            "Should have transferred at least 5 entities"
        );
    }

    /// 4-node scenario mimicking the fuzzy test with initialized nodes.
    ///
    /// All nodes share a base entity. Node 1 then writes seed data.
    /// After sync rounds, all should converge.
    #[tokio::test]
    async fn test_four_node_seed_data_propagation() {
        let ctx = shared_context();
        let mut node1 = SimNode::new_in_context("node1", ctx);
        let mut node2 = SimNode::new_in_context("node2", ctx);
        let mut node3 = SimNode::new_in_context("node3", ctx);
        let mut node4 = SimNode::new_in_context("node4", ctx);

        // All share a base entity (all initialized)
        let base_id = EntityId::from_u64(1000);
        for node in [&mut node1, &mut node2, &mut node3, &mut node4] {
            node.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        }

        // Node 1 writes 10 seed values (exactly like the fuzzy test)
        for i in 1..=10 {
            node1.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("seed-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        assert_eq!(node1.entity_count(), 11);
        assert_eq!(node2.entity_count(), 1);
        assert_eq!(node3.entity_count(), 1);
        assert_eq!(node4.entity_count(), 1);

        // Node 1 initiates sync with each other node (push direction)
        execute_hash_comparison_sync(&mut node1, &node2)
            .await
            .expect("n1<->n2");
        execute_hash_comparison_sync(&mut node1, &node3)
            .await
            .expect("n1<->n3");
        execute_hash_comparison_sync(&mut node1, &node4)
            .await
            .expect("n1<->n4");

        // All nodes should have the seed data after one round
        assert_eq!(
            node2.entity_count(),
            11,
            "Node 2 should have 11 entities after sync with node 1"
        );
        assert_eq!(
            node3.entity_count(),
            11,
            "Node 3 should have 11 entities after sync with node 1"
        );
        assert_eq!(
            node4.entity_count(),
            11,
            "Node 4 should have 11 entities after sync with node 1"
        );

        // All should converge to same root hash
        assert_eq!(node1.root_hash(), node2.root_hash());
        assert_eq!(node1.root_hash(), node3.root_hash());
        assert_eq!(node1.root_hash(), node4.root_hash());
    }

    /// Minimal reproduction: initiator has 1 extra entity beyond shared base.
    #[tokio::test]
    async fn test_single_entity_push_with_shared_base() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());

        // Alice adds one extra entity
        alice.insert_entity_with_metadata(
            EntityId::from_u64(42),
            b"hello".to_vec(),
            EntityMetadata::default(),
        );

        assert_eq!(alice.entity_count(), 2);
        assert_eq!(bob.entity_count(), 1);

        let stats = execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        assert_eq!(bob.entity_count(), 2, "Bob should have 2 entities");
        assert_eq!(alice.root_hash(), bob.root_hash());
        assert!(
            stats.entities_pushed >= 1,
            "Should have pushed at least 1 entity, got {}",
            stats.entities_pushed,
        );
    }

    /// CRDT conflict during push: alice pushes entity that bob already has
    /// with a different value. LWW merge should resolve deterministically.
    #[tokio::test]
    async fn test_crdt_conflict_during_push() {
        use calimero_primitives::crdt::CrdtType;

        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());

        // Both have the conflict entity, different values and timestamps
        let conflict_id = EntityId::from_u64(42);
        alice.insert_entity_with_metadata(
            conflict_id,
            b"alice-wins".to_vec(),
            EntityMetadata::new(CrdtType::lww_register("test"), 200), // newer
        );
        bob.insert_entity_with_metadata(
            conflict_id,
            b"bob-loses".to_vec(),
            EntityMetadata::new(CrdtType::lww_register("test"), 100), // older
        );

        // Alice also has extra entities to trigger push
        for i in 1..=3 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("alice-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        assert_ne!(alice.root_hash(), bob.root_hash());

        // Round 1: Alice initiates → pushes her data to bob, pulls bob's
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("a->b should succeed");

        // Round 2: Bob initiates → pushes merged state back to alice
        execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("b->a should succeed");

        // After two rounds, both should converge
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "Should converge after bidirectional CRDT merge"
        );

        // Both should have: 1 base + 1 conflict (merged) + 3 unique = 5
        assert_eq!(alice.entity_count(), 5);
        assert_eq!(bob.entity_count(), 5);
    }

    /// Symmetric sync: A→B then B→A should produce identical state.
    #[tokio::test]
    async fn test_symmetric_sync_converges() {
        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());

        // Each has unique data
        for i in 1..=5 {
            alice.insert_entity_with_metadata(
                EntityId::from_u64(i),
                format!("a-{i}").into_bytes(),
                EntityMetadata::default(),
            );
            bob.insert_entity_with_metadata(
                EntityId::from_u64(100 + i),
                format!("b-{i}").into_bytes(),
                EntityMetadata::default(),
            );
        }

        // A→B: alice initiates, pushes her data, pulls bob's
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("a->b");

        // Should already converge after one bidirectional sync
        assert_eq!(alice.entity_count(), 11); // 1 base + 5 alice + 5 bob
        assert_eq!(bob.entity_count(), 11);
        assert_eq!(alice.root_hash(), bob.root_hash());

        // B→A: bob initiates (should be no-op since already converged)
        let stats = execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("b->a");

        assert_eq!(stats.entities_transferred, 0, "no-op when already synced");
        assert_eq!(stats.entities_pushed, 0, "no-op when already synced");
        assert_eq!(alice.root_hash(), bob.root_hash());
    }

    /// **REGRESSION GUARD (opaque-leaf sync)**: a Merkle leaf with no `crdt_type`
    /// — the `Root<T>` app-state entry `Id::new([118; 32])` — present on one node
    /// but not the other must reconcile via HashComparison so both converge.
    ///
    /// Before the fix, `get_local_tree_node` returned a malformed `internal` node
    /// (empty children) for an opaque leaf and `collect_leaves_recursive` skipped
    /// it, so the entity was never pushed/pulled and the two nodes' Merkle root
    /// hashes stayed divergent forever.
    #[tokio::test]
    async fn test_opaque_leaf_converges_via_hash_comparison() {
        use calimero_storage::address::Id;
        use calimero_storage::entities::Metadata;

        // `Id::new([118; 32])` == `Root::<T>::entry_id()`.
        const ROOT_ENTRY_ID: [u8; 32] = [118u8; 32];

        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base entity so both nodes are "initialized" (non-zero root hash),
        // mirroring the real scenario where HashComparison (not Snapshot) runs.
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        assert_eq!(alice.root_hash(), bob.root_hash());

        // Alice has the `Root<T>` entry — a leaf with NO crdt_type (opaque).
        // Seeded directly via storage so `crdt_type` stays `None`.
        let opaque_value = b"app-root-state-v1".to_vec();
        alice.storage().add_entity(
            Id::new(ROOT_ENTRY_ID),
            &opaque_value,
            Metadata::new(100, 100),
        );

        // Sanity: Alice's opaque entity is genuinely opaque.
        let alice_idx = alice
            .storage()
            .get_index(Id::new(ROOT_ENTRY_ID))
            .expect("alice should have the opaque entity");
        assert!(
            alice_idx.metadata.crdt_type.is_none(),
            "seeded entity must have crdt_type == None"
        );

        // Bob does not have it → diverged.
        assert!(bob
            .storage()
            .get_entity_data(Id::new(ROOT_ENTRY_ID))
            .is_none());
        assert_ne!(
            alice.root_hash(),
            bob.root_hash(),
            "nodes should be diverged on the opaque leaf"
        );

        // Alice initiates HashComparison sync with Bob.
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        // (a) Bob now has the same entity bytes.
        assert_eq!(
            bob.storage()
                .get_entity_data(Id::new(ROOT_ENTRY_ID))
                .as_deref(),
            Some(opaque_value.as_slice()),
            "Bob should have the opaque entity bytes after sync"
        );
        // (b) Merkle root hashes converged.
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "root hashes should converge after syncing the opaque leaf"
        );
    }

    /// **REGRESSION GUARD (opaque-leaf sync, responder side)**: the mirror of
    /// `test_opaque_leaf_converges_via_hash_comparison` — here the opaque leaf
    /// (a no-`crdt_type` entity) lives on the *responder* (Bob) and the
    /// initiator (Alice) lacks it, so Alice must *pull* it via the tree-walk:
    /// Bob's `get_local_tree_node` must emit the opaque entity as a real leaf,
    /// and Alice's initiator applies it through the `remote_node.is_valid()`
    /// guard and `apply_leaf_with_crdt_merge`.
    ///
    /// Before the fix, Bob's `get_local_tree_node` returned a malformed
    /// `internal` node (empty children) for the opaque leaf, which Alice's
    /// initiator drops as an invalid `TreeNode`, so the entity was never pulled
    /// and the two nodes' Merkle root hashes stayed divergent forever.
    #[tokio::test]
    async fn test_opaque_leaf_on_responder_converges_via_hash_comparison() {
        use calimero_storage::address::Id;
        use calimero_storage::entities::Metadata;

        // `Id::new([118; 32])` == `Root::<T>::entry_id()`.
        const ROOT_ENTRY_ID: [u8; 32] = [118u8; 32];

        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);

        // Shared base entity so both nodes are "initialized" (non-zero root hash),
        // mirroring the real scenario where HashComparison (not Snapshot) runs.
        let base_id = EntityId::from_u64(1000);
        alice.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        bob.insert_entity_with_metadata(base_id, b"base".to_vec(), EntityMetadata::default());
        assert_eq!(alice.root_hash(), bob.root_hash());

        // Bob has the `Root<T>` entry — a leaf with NO crdt_type (opaque).
        // Seeded directly via storage so `crdt_type` stays `None`.
        let opaque_value = b"app-root-state-v1".to_vec();
        bob.storage().add_entity(
            Id::new(ROOT_ENTRY_ID),
            &opaque_value,
            Metadata::new(100, 100),
        );

        // Sanity: Bob's opaque entity is genuinely opaque.
        let bob_idx = bob
            .storage()
            .get_index(Id::new(ROOT_ENTRY_ID))
            .expect("bob should have the opaque entity");
        assert!(
            bob_idx.metadata.crdt_type.is_none(),
            "seeded entity must have crdt_type == None"
        );

        // Alice does not have it → diverged.
        assert!(alice
            .storage()
            .get_entity_data(Id::new(ROOT_ENTRY_ID))
            .is_none());
        assert_ne!(
            alice.root_hash(),
            bob.root_hash(),
            "nodes should be diverged on the opaque leaf"
        );

        // Alice initiates HashComparison sync with Bob → Alice must *pull* the
        // opaque leaf via the tree-walk.
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        // (a) Alice now has the same entity bytes.
        assert_eq!(
            alice
                .storage()
                .get_entity_data(Id::new(ROOT_ENTRY_ID))
                .as_deref(),
            Some(opaque_value.as_slice()),
            "Alice should have Bob's opaque entity bytes after sync"
        );
        // (b) Merkle root hashes converged.
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "root hashes should converge after pulling the opaque leaf"
        );
    }

    /// **REGRESSION GUARD (nested-collection convergence)**: when a leaf
    /// pushed via HashComparison's `EntityPush` lives *under a non-root
    /// parent* (e.g. `Root<KvStore>::items["k"]` is a child of the items
    /// collection, not a direct child of the context root), the receiver
    /// must place it at the SAME Merkle position the sender has — not
    /// arbitrarily as a child of the context root.
    ///
    /// Pre-fix, `apply_leaf_with_crdt_merge` always attached a new
    /// `Action::Add` to `ChildInfo::new(context_root, …)`. For any
    /// non-root-direct-child entity, the receiver's local copy ended up
    /// at a different Merkle position than the sender's, and the two
    /// nodes' parent (and root) hashes diverged irreconcilably. The
    /// observable signature was 38+ identical-stat HashComparison
    /// sessions in a row on the receiver (each one "merging" the same
    /// entities) with the root hash never converging — see the smoke-
    /// test Round-2 failure on `bdc61af`.
    ///
    /// The fix carries the sender's `index.parent_id()` on the wire via
    /// the (already-defined-but-unpopulated) `LeafMetadata.parent_id`
    /// field, and the receiver uses it as the ancestor in `Action::Add`.
    ///
    /// This test mirrors the production cross-node-writes scenario: both
    /// nodes share a parent entity AND each have a sibling child under it
    /// (so the parent is "internal" on both sides — analogous to the
    /// items collection of a kv-store that already has prior entries).
    /// Then Alice adds a NEW child under the parent. After HashComparison
    /// sync, Bob must have that new child under the same parent and the
    /// root hashes must converge.
    ///
    /// Pre-fix, Bob's `apply_leaf_with_crdt_merge` ignored
    /// `leaf.metadata.parent_id` (it was always `None` on the wire) and
    /// attached the new child to context root. So Bob's parent kept its
    /// single original child, while Alice's parent had two children →
    /// different parent hashes → different root hashes, forever.
    #[tokio::test]
    async fn test_nested_leaf_converges_at_correct_merkle_position() {
        use calimero_primitives::crdt::CrdtType;
        use calimero_storage::address::Id;
        use calimero_storage::entities::Metadata;

        let ctx = shared_context();
        let mut alice = SimNode::new_in_context("alice", ctx);
        let bob = SimNode::new_in_context("bob", ctx);

        // Both nodes share the parent + one initial child under it. After
        // these inserts both nodes have an identical 2-level tree: parent
        // (with one child) under context root. Same data, same metadata
        // → same `own_hash` and `full_hash` everywhere; root hashes match.
        let parent_storage_id = Id::new([1u8; 32]);
        let initial_child_id = Id::new([2u8; 32]);
        let mut child_meta = Metadata::new(50, 50);
        child_meta.crdt_type = Some(CrdtType::lww_register("alloc::string::String"));
        for node in [&alice, &bob] {
            node.storage()
                .add_entity(parent_storage_id, b"shared-parent", Metadata::new(50, 50));
            node.storage().add_entity_with_parent(
                initial_child_id,
                parent_storage_id,
                b"shared-child",
                child_meta.clone(),
            );
        }
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "nodes start at the same root with parent + shared child"
        );

        // Alice adds a NEW child under the same parent — analogous to a
        // local `kv.set("gamma", ...)` after both nodes already have
        // `kv.set("alpha", ...)` settled. This is the topology where
        // `LeafMetadata.parent_id` must be honoured on push.
        let new_child_id = Id::new([200u8; 32]);
        let mut new_child_meta = Metadata::new(200, 200);
        new_child_meta.crdt_type = Some(CrdtType::lww_register("alloc::string::String"));
        let new_child_value = b"value-from-alice".to_vec();
        alice.storage().add_entity_with_parent(
            new_child_id,
            parent_storage_id,
            &new_child_value,
            new_child_meta,
        );

        // Alice's local topology: new child under the shared parent.
        let alice_new = alice
            .storage()
            .get_index(new_child_id)
            .expect("alice should have the new child");
        assert_eq!(
            alice_new.parent_id(),
            Some(parent_storage_id),
            "alice's new child must be parented to the shared parent, not context root"
        );

        // Bob doesn't have it yet → diverged.
        assert!(bob.storage().get_entity_data(new_child_id).is_none());
        assert_ne!(
            alice.root_hash(),
            bob.root_hash(),
            "nodes should be diverged on the new child"
        );

        // Alice initiates HashComparison sync with Bob.
        execute_hash_comparison_sync(&mut alice, &bob)
            .await
            .expect("sync should succeed");

        // (a) Bob has the new child's bytes.
        assert_eq!(
            bob.storage().get_entity_data(new_child_id).as_deref(),
            Some(new_child_value.as_slice()),
            "Bob should have the new child's bytes after sync"
        );

        // (b) **The structural assertion**: Bob's new child is parented
        // to the shared parent, not to context root. Pre-fix this
        // assertion fails — `apply_leaf_with_crdt_merge` placed the
        // entity as a direct child of context root.
        let bob_new = bob
            .storage()
            .get_index(new_child_id)
            .expect("bob should have the new child");
        assert_eq!(
            bob_new.parent_id(),
            Some(parent_storage_id),
            "Bob's new child must be parented to the shared parent (the sender's parent), \
             not to context root — `LeafMetadata.parent_id` must be honoured by the apply \
             path. A failure here is the 'Same DAG heads, different root hash' bug under \
             cross-node writes to a non-root collection (bdc61af Round-2 failure)."
        );

        // (c) Merkle root hashes converged. With the wrong parent on Bob,
        // his parent (and therefore root) hash would differ from Alice's
        // even when all leaf bytes match — this catches that divergence.
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "root hashes should converge — new child placed at correct Merkle position"
        );
    }
}
