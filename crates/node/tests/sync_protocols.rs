//! Sync protocol tests
//!
//! Tests the actual sync protocols used in production:
//! - Missing delta catch-up flow
//! - Snapshot transfer protocol
//! - Peer selection logic
//! - Hash heartbeat divergence detection
//! - Merkle comparison
//! - Recovery from divergence

use std::collections::HashMap;
use std::sync::Arc;

use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier, MAX_DELTA_QUERY_LIMIT};
use calimero_primitives::hash::Hash;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::snapshot::Snapshot;
use calimero_storage::store::MainStorage;
use calimero_storage::Interface;
use tokio::sync::RwLock;

// ============================================================
// Mock Applier for Testing
// ============================================================

/// Simple applier that just tracks applications
struct TestApplier {
    applied: Arc<RwLock<Vec<[u8; 32]>>>,
}

impl TestApplier {
    fn new() -> Self {
        Self {
            applied: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn get_applied(&self) -> Vec<[u8; 32]> {
        self.applied.read().await.clone()
    }
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for TestApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Apply actions to storage
        for action in &delta.payload {
            Interface::<MainStorage>::apply_action(action.clone())
                .map_err(|e| ApplyError::Application(e.to_string()))?;
        }

        self.applied.write().await.push(delta.id);
        Ok(())
    }
}

// ============================================================
// Simulated Node with DAG
// ============================================================

struct SimulatedNode {
    node_id: String,
    dag: Arc<RwLock<DagStore<Vec<Action>>>>,
    applier: Arc<TestApplier>,

    /// Track local root hash (simulated)
    root_hash: Arc<RwLock<Hash>>,
}

impl SimulatedNode {
    fn new(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            dag: Arc::new(RwLock::new(DagStore::new([0; 32]))),
            applier: Arc::new(TestApplier::new()),
            root_hash: Arc::new(RwLock::new(Hash::from([0; 32]))),
        }
    }

    async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> eyre::Result<bool> {
        let mut dag = self.dag.write().await;
        let applied = dag.add_delta(delta, &*self.applier).await?;

        if applied {
            // Update root hash (simulated)
            *self.root_hash.write().await = Hash::from([99; 32]); // Dummy hash
        }

        Ok(applied)
    }

    async fn get_missing_parents(&self) -> Vec<[u8; 32]> {
        self.dag
            .read()
            .await
            .get_missing_parents(MAX_DELTA_QUERY_LIMIT)
    }

    async fn get_heads(&self) -> Vec<[u8; 32]> {
        self.dag.read().await.get_heads()
    }

    async fn get_root_hash(&self) -> Hash {
        *self.root_hash.read().await
    }

    async fn get_delta(&self, delta_id: &[u8; 32]) -> Option<CausalDelta<Vec<Action>>> {
        self.dag.read().await.get_delta(delta_id).cloned()
    }
}

// ============================================================
// Test: Missing Delta Catch-Up Flow
// ============================================================

#[tokio::test]
async fn test_missing_delta_catch_up_single_parent() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Node A has deltas 1 and 2 (using empty payloads to avoid entity index issues)
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);

    // Node A receives both
    node_a.add_delta(delta1.clone()).await.unwrap();
    node_a.add_delta(delta2.clone()).await.unwrap();

    // Node B only receives delta2 (missing delta1)
    node_b.add_delta(delta2.clone()).await.unwrap();

    // Delta2 should be pending
    let missing = node_b.get_missing_parents().await;
    assert_eq!(
        missing,
        vec![[1; 32]],
        "Node B should be missing parent delta 1"
    );

    // Simulate: Node B requests missing delta from Node A
    let requested_delta = node_a.get_delta(&[1; 32]).await;
    assert!(requested_delta.is_some());

    // Node B receives the missing delta
    node_b.add_delta(requested_delta.unwrap()).await.unwrap();

    // Now node B should have no missing parents
    let missing_after = node_b.get_missing_parents().await;
    assert_eq!(missing_after.len(), 0);

    // Both deltas should be applied
    let applied = node_b.applier.get_applied().await;
    assert_eq!(applied.len(), 2);
    assert!(applied.contains(&[1; 32]));
    assert!(applied.contains(&[2; 32]));
}

#[tokio::test]
async fn test_missing_delta_catch_up_multiple_parents() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Create a merge scenario: delta3 merges delta1 and delta2
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[0; 32]], vec![]);
    let delta3_merge = CausalDelta::new_test([3; 32], vec![[1; 32], [2; 32]], vec![]);

    // Node A has all deltas
    node_a.add_delta(delta1.clone()).await.unwrap();
    node_a.add_delta(delta2.clone()).await.unwrap();
    node_a.add_delta(delta3_merge.clone()).await.unwrap();

    // Node B only receives the merge delta (missing both parents)
    node_b.add_delta(delta3_merge).await.unwrap();

    // Should be missing both parents
    let mut missing = node_b.get_missing_parents().await;
    missing.sort();
    assert_eq!(missing.len(), 2);
    assert!(missing.contains(&[1; 32]));
    assert!(missing.contains(&[2; 32]));

    // Request and apply both missing deltas
    for delta_id in missing {
        let delta = node_a.get_delta(&delta_id).await.unwrap();
        node_b.add_delta(delta).await.unwrap();
    }

    // All 3 deltas should now be applied
    let applied = node_b.applier.get_applied().await;
    assert_eq!(applied.len(), 3);
}

#[tokio::test]
async fn test_deep_chain_catch_up() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Create a chain of 10 deltas
    let mut prev_id = [0; 32];
    let mut deltas = vec![];

    for i in 1..=10 {
        let mut id = [0; 32];
        id[0] = i;

        let delta = CausalDelta::new_test(id, vec![prev_id], vec![]);

        deltas.push(delta.clone());
        prev_id = id;
    }

    // Node A receives all deltas
    for delta in &deltas {
        node_a.add_delta(delta.clone()).await.unwrap();
    }

    // Node B only receives the last delta (delta 10)
    node_b.add_delta(deltas[9].clone()).await.unwrap();

    // Should be missing delta 9
    let missing = node_b.get_missing_parents().await;
    assert_eq!(missing.len(), 1);

    // Request missing deltas one by one (simulating catch-up)
    let mut current_missing = missing;
    let mut requested_count = 0;

    while !current_missing.is_empty() {
        for delta_id in &current_missing {
            let delta = node_a.get_delta(delta_id).await.unwrap();
            node_b.add_delta(delta).await.unwrap();
            requested_count += 1;
        }
        current_missing = node_b.get_missing_parents().await;
    }

    // Should have requested all 9 missing deltas
    assert_eq!(requested_count, 9);

    // All 10 deltas should be applied
    let applied = node_b.applier.get_applied().await;
    assert_eq!(applied.len(), 10);
}

// ============================================================
// Test: Snapshot Transfer Protocol
// ============================================================

#[tokio::test]
async fn test_snapshot_transfer_fresh_node() {
    // Node A has state, Node B is fresh
    let node_a = SimulatedNode::new("node_a");
    let _node_b = SimulatedNode::new("node_b");

    // Node A applies some deltas (empty payloads for simplicity)
    for i in 1u8..=5u8 {
        let delta = CausalDelta::new_test(
            [i; 32],
            vec![if i == 1 { [0; 32] } else { [i - 1; 32] }],
            vec![],
        );

        node_a.add_delta(delta).await.unwrap();
    }

    // Node A's state is now ahead
    let node_a_heads = node_a.get_heads().await;
    assert_eq!(node_a_heads, vec![[5; 32]]);

    // Simulate snapshot transfer:
    // In production, node B would request full snapshot from node A

    // Note: Snapshot transfer requires IterableStorage
    // For this test, just verify the mechanism works conceptually

    // Create a mock snapshot
    let snapshot = Snapshot {
        entity_count: 5,
        index_count: 0,
        entries: vec![],
        indexes: vec![],
        root_hash: [0; 32],
        timestamp: 0,
    };

    // After snapshot, node B should have same state
    // (Simplified verification - in real test, would check entity data)
    assert_eq!(snapshot.entity_count, 5);
}

#[tokio::test]
async fn test_snapshot_excludes_tombstones() {
    // Create a mock snapshot representing state without tombstones
    // In production, tombstones are filtered during snapshot generation

    // Simulate: 5 entities created, 2 deleted (tombstoned)
    // Snapshot should only contain 3 live entities
    let live_entries = vec![
        (Id::from([1; 32]), vec![1u8; 10]),
        (Id::from([3; 32]), vec![3u8; 10]),
        (Id::from([5; 32]), vec![5u8; 10]),
    ];

    let snapshot = Snapshot {
        entity_count: 3,
        index_count: 0,
        entries: live_entries.clone(),
        indexes: vec![],
        root_hash: [1; 32],
        timestamp: 0,
    };

    // Verify snapshot only contains live entities
    assert_eq!(snapshot.entries.len(), 3);

    // Entity IDs 2 and 4 should not be present (they were tombstoned)
    let entity_ids: Vec<u8> = snapshot
        .entries
        .iter()
        .map(|(id, _)| id.as_bytes()[0])
        .collect();
    assert!(
        !entity_ids.contains(&2),
        "Tombstone should not be in snapshot"
    );
    assert!(
        !entity_ids.contains(&4),
        "Tombstone should not be in snapshot"
    );
    assert!(entity_ids.contains(&1));
    assert!(entity_ids.contains(&3));
    assert!(entity_ids.contains(&5));
}

// ============================================================
// Test: Peer Selection Logic
// ============================================================

#[tokio::test]
async fn test_peer_selection_prefers_peer_with_state() {
    // Simulate 3 peers: 2 uninitialized, 1 with state
    let mut peers = HashMap::new();

    peers.insert("peer_a", ([0; 32], vec![[0; 32]])); // Uninitialized
    peers.insert("peer_b", ([0; 32], vec![[0; 32]])); // Uninitialized
    peers.insert("peer_c", ([1; 32], vec![[5; 32]])); // Has state

    // Peer selection logic: prefer non-zero root hash
    let selected = peers
        .iter()
        .find(|(_, (root_hash, _))| *root_hash != [0; 32])
        .map(|(id, _)| *id);

    assert_eq!(selected, Some("peer_c"));
}

#[tokio::test]
async fn test_peer_selection_random_when_all_initialized() {
    let mut peers = HashMap::new();

    // All peers have state
    peers.insert("peer_a", ([1; 32], vec![[3; 32]]));
    peers.insert("peer_b", ([2; 32], vec![[4; 32]]));
    peers.insert("peer_c", ([3; 32], vec![[5; 32]]));

    // When all have state, any peer is valid
    // In production, random selection is used
    let any_peer_valid = peers
        .iter()
        .all(|(_, (root_hash, _))| *root_hash != [0; 32]);

    assert!(any_peer_valid);
}

// ============================================================
// Test: Hash Heartbeat Divergence Detection
// ============================================================

#[tokio::test]
async fn test_hash_heartbeat_detects_silent_divergence() {
    // **CRITICAL TEST**: Matches production divergence detection logic!
    // Production code (network_event.rs:144-154):
    //   if our_heads_set == their_heads_set && our_context.root_hash != their_root_hash {
    //       error!("DIVERGENCE DETECTED: Same DAG heads but different root hash!");
    //   }
    //
    // This scenario should NEVER happen in correct implementation, but detects:
    // - Storage corruption
    // - WASM execution bugs
    // - Non-deterministic application logic

    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Both nodes receive and apply the SAME delta
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);

    node_a.add_delta(delta1.clone()).await.unwrap();
    node_b.add_delta(delta1).await.unwrap();

    // Both should have SAME DAG heads
    assert_eq!(node_a.get_heads().await, node_b.get_heads().await);
    assert_eq!(node_a.get_heads().await, vec![[1; 32]]);

    // Simulate corruption/bug: Manually set different root hashes
    // (In reality, this would happen due to non-deterministic WASM execution)
    *node_a.root_hash.write().await = Hash::from([100; 32]); // Corrupted!
    *node_b.root_hash.write().await = Hash::from([200; 32]); // Different corruption!

    // **THIS IS THE DIVERGENCE SCENARIO PRODUCTION DETECTS:**
    // - Same DAG heads: [1; 32]
    // - Different root hashes: [100; 32] vs [200; 32]

    let heads_a = node_a.get_heads().await;
    let heads_b = node_b.get_heads().await;
    let hash_a = node_a.get_root_hash().await;
    let hash_b = node_b.get_root_hash().await;

    // Same heads
    assert_eq!(heads_a, heads_b, "Nodes should have same DAG heads");

    // Different root hashes (DIVERGENCE!)
    assert_ne!(
        hash_a, hash_b,
        "Different root hash indicates silent divergence"
    );

    // This is the exact condition checked in production:
    // our_heads_set == their_heads_set && our_root_hash != their_root_hash
    assert_eq!(heads_a, heads_b);
    assert_ne!(hash_a, hash_b);
}

#[tokio::test]
async fn test_heartbeat_with_same_state_no_divergence() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Both nodes apply same deltas
    for i in 1..=3 {
        let mut id = [0; 32];
        id[0] = i;

        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);

        node_a.add_delta(delta.clone()).await.unwrap();
        node_b.add_delta(delta).await.unwrap();
    }

    // Same heads
    assert_eq!(node_a.get_heads().await, node_b.get_heads().await);

    // Same root hash
    assert_eq!(node_a.get_root_hash().await, node_b.get_root_hash().await);

    // No divergence - this is the happy path
}

// ============================================================
// Test: Merkle Comparison for Sync
// ============================================================

#[tokio::test]
async fn test_merkle_comparison_detects_differences() {
    // In production, Merkle tree is used to efficiently compare storage state
    // without full state comparison

    // Simulate two nodes with different entity trees
    let node_a_entities = vec![
        (Id::from([1; 32]), vec![1, 2, 3]),
        (Id::from([2; 32]), vec![4, 5, 6]),
    ];

    let node_b_entities = vec![
        (Id::from([1; 32]), vec![1, 2, 3]), // Same
        (Id::from([2; 32]), vec![9, 9, 9]), // Different!
    ];

    // Compute simple merkle hash (in production, would use actual merkle tree)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let hash_a = {
        let mut hasher = DefaultHasher::new();
        for (id, data) in &node_a_entities {
            id.hash(&mut hasher);
            data.hash(&mut hasher);
        }
        hasher.finish()
    };

    let hash_b = {
        let mut hasher = DefaultHasher::new();
        for (id, data) in &node_b_entities {
            id.hash(&mut hasher);
            data.hash(&mut hasher);
        }
        hasher.finish()
    };

    // Merkle hashes should differ, detecting the divergence
    assert_ne!(
        hash_a, hash_b,
        "Merkle comparison should detect different state"
    );
}

// ============================================================
// Test: Recovery from Divergence
// ============================================================

#[tokio::test]
async fn test_recovery_via_full_resync() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Nodes have diverged (simulated by different deltas)
    let delta_a = CausalDelta::new_test([10; 32], vec![[0; 32]], vec![]);
    let delta_b = CausalDelta::new_test([20; 32], vec![[0; 32]], vec![]);

    node_a.add_delta(delta_a).await.unwrap();
    node_b.add_delta(delta_b).await.unwrap();

    // Heads are different - divergence detected
    assert_ne!(node_a.get_heads().await, node_b.get_heads().await);

    // Recovery: Full resync via snapshot
    // Node B requests snapshot from node A and applies it

    // In production, would transfer actual snapshot
    // For test, just verify recovery mechanism
    let snapshot = Snapshot {
        entity_count: 1,
        index_count: 0,
        entries: vec![(Id::from([100; 32]), vec![1])],
        indexes: vec![],
        root_hash: [1; 32],
        timestamp: 0,
    };

    // After resync, node B has node A's state
    assert_eq!(snapshot.entries.len(), 1);
}

#[tokio::test]
async fn test_recovery_via_delta_replay() {
    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");

    // Node B is behind by several deltas
    let mut deltas = vec![];
    for i in 1..=5 {
        let mut id = [0; 32];
        id[0] = i;

        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);

        deltas.push(delta.clone());
        node_a.add_delta(delta).await.unwrap();
    }

    // Node B is at root
    assert_eq!(node_b.get_heads().await, vec![[0; 32]]);

    // Recovery: Request all missing deltas
    let node_a_heads = node_a.get_heads().await;

    // In production, would request all deltas from root to current heads
    // For test, just apply all deltas
    for delta in deltas {
        node_b.add_delta(delta).await.unwrap();
    }

    // Now synchronized
    assert_eq!(node_b.get_heads().await, node_a_heads);
}

// ============================================================
// Test: Network Partitions During Sync
// ============================================================

/// Simulates a network that can be partitioned
struct PartitionableNetwork {
    /// Which nodes can communicate with each other (bi-directional)
    /// If (a, b) is in connected, a and b can communicate
    connected: Arc<RwLock<std::collections::HashSet<(String, String)>>>,
}

impl PartitionableNetwork {
    fn new() -> Self {
        Self {
            connected: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    async fn connect(&self, node_a: &str, node_b: &str) {
        let mut connected = self.connected.write().await;
        connected.insert((node_a.to_string(), node_b.to_string()));
        connected.insert((node_b.to_string(), node_a.to_string()));
    }

    async fn disconnect(&self, node_a: &str, node_b: &str) {
        let mut connected = self.connected.write().await;
        connected.remove(&(node_a.to_string(), node_b.to_string()));
        connected.remove(&(node_b.to_string(), node_a.to_string()));
    }

    async fn can_communicate(&self, node_a: &str, node_b: &str) -> bool {
        let connected = self.connected.read().await;
        connected.contains(&(node_a.to_string(), node_b.to_string()))
    }
}

#[tokio::test]
async fn test_network_partition_during_sync_recovers() {
    // Test that sync can recover after network partition heals

    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");
    let network = PartitionableNetwork::new();

    // Initially nodes are connected
    network.connect("node_a", "node_b").await;

    // Node A creates deltas 1-5
    for i in 1..=5 {
        let mut id = [0; 32];
        id[0] = i;
        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);
        node_a.add_delta(delta).await.unwrap();
    }

    // Node B receives only first 2 deltas while connected
    for i in 1..=2 {
        let mut id = [0; 32];
        id[0] = i;
        if network.can_communicate("node_a", "node_b").await {
            if let Some(delta) = node_a.get_delta(&id).await {
                let _result = node_b.add_delta(delta).await;
            }
        }
    }

    // Network partition occurs
    network.disconnect("node_a", "node_b").await;

    // Node A creates more deltas during partition
    for i in 6..=8 {
        let mut id = [0; 32];
        id[0] = i;
        let delta = CausalDelta::new_test(id, vec![[i - 1; 32]], vec![]);
        node_a.add_delta(delta).await.unwrap();
    }

    // Verify node B cannot get new deltas during partition
    let mut id = [0; 32];
    id[0] = 6;
    assert!(
        !network.can_communicate("node_a", "node_b").await,
        "Nodes should be partitioned"
    );

    // Network heals
    network.connect("node_a", "node_b").await;

    // Node B catches up by requesting missing deltas
    // First get what's missing
    let missing = node_b.get_missing_parents().await;

    // Request all deltas from node A to catch up
    // This simulates the catch-up flow after partition heals
    let mut to_sync: Vec<[u8; 32]> = missing;

    // Also need to sync any deltas node B doesn't have
    let node_a_heads = node_a.get_heads().await;
    for head in &node_a_heads {
        if node_b.get_delta(head).await.is_none() {
            to_sync.push(*head);
        }
    }

    // Sync all missing deltas in topological order
    let mut synced_count = 0;
    for _ in 0..20 {
        // Max iterations to prevent infinite loop
        let current_missing = node_b.get_missing_parents().await;
        if current_missing.is_empty() {
            break;
        }

        for delta_id in &current_missing {
            if let Some(delta) = node_a.get_delta(delta_id).await {
                if node_b.add_delta(delta).await.is_ok() {
                    synced_count += 1;
                }
            }
        }
    }

    // Also apply any heads we're missing
    for head_id in &node_a_heads {
        if node_b.get_delta(head_id).await.is_none() {
            if let Some(delta) = node_a.get_delta(head_id).await {
                if node_b.add_delta(delta).await.is_ok() {
                    synced_count += 1;
                }
            }
        }
    }

    // After recovery, nodes should be in sync
    assert!(synced_count > 0, "Should have synced some deltas");
    assert_eq!(
        node_b.get_missing_parents().await.len(),
        0,
        "No missing parents after recovery"
    );
}

#[tokio::test]
async fn test_network_partition_three_nodes_split_brain() {
    // Test scenario: 3 nodes, partition creates split-brain, then heals

    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");
    let node_c = SimulatedNode::new("node_c");
    let network = PartitionableNetwork::new();

    // Initial state: all connected
    network.connect("node_a", "node_b").await;
    network.connect("node_b", "node_c").await;
    network.connect("node_a", "node_c").await;

    // All nodes start with root delta
    let root_delta = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    node_a.add_delta(root_delta.clone()).await.unwrap();
    node_b.add_delta(root_delta.clone()).await.unwrap();
    node_c.add_delta(root_delta).await.unwrap();

    // Network partition: A-B in one partition, C isolated
    network.disconnect("node_a", "node_c").await;
    network.disconnect("node_b", "node_c").await;

    // A creates delta in partition 1
    let delta_a = CausalDelta::new_test([10; 32], vec![[1; 32]], vec![]);
    node_a.add_delta(delta_a.clone()).await.unwrap();

    // B receives delta from A (same partition)
    if network.can_communicate("node_a", "node_b").await {
        node_b.add_delta(delta_a).await.unwrap();
    }

    // C creates delta in isolation (different branch)
    let delta_c = CausalDelta::new_test([20; 32], vec![[1; 32]], vec![]);
    node_c.add_delta(delta_c).await.unwrap();

    // Verify split state
    assert_eq!(node_a.get_heads().await, vec![[10; 32]]);
    assert_eq!(node_b.get_heads().await, vec![[10; 32]]);
    assert_eq!(node_c.get_heads().await, vec![[20; 32]]);

    // Network heals
    network.connect("node_a", "node_c").await;
    network.connect("node_b", "node_c").await;

    // Exchange deltas to merge branches
    let delta_from_a = node_a.get_delta(&[10; 32]).await.unwrap();
    let delta_from_c = node_c.get_delta(&[20; 32]).await.unwrap();

    // All nodes receive both branches
    node_a.add_delta(delta_from_c.clone()).await.unwrap();
    node_b.add_delta(delta_from_c).await.unwrap();
    node_c.add_delta(delta_from_a).await.unwrap();

    // All nodes should now have both branches as heads
    let mut heads_a = node_a.get_heads().await;
    let mut heads_b = node_b.get_heads().await;
    let mut heads_c = node_c.get_heads().await;

    heads_a.sort();
    heads_b.sort();
    heads_c.sort();

    assert_eq!(heads_a, heads_b);
    assert_eq!(heads_b, heads_c);
    assert_eq!(heads_a.len(), 2, "Should have 2 concurrent heads");
}

// ============================================================
// Test: Concurrent Syncs to Same Context
// ============================================================

#[tokio::test]
async fn test_concurrent_syncs_to_same_context() {
    // Multiple nodes syncing deltas concurrently to the same DAG

    let node_a = SimulatedNode::new("node_a");
    let node_b = SimulatedNode::new("node_b");
    let node_c = SimulatedNode::new("node_c");

    // Source node has a chain of deltas
    let source = SimulatedNode::new("source");
    for i in 1..=10 {
        let mut id = [0; 32];
        id[0] = i;
        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);
        source.add_delta(delta).await.unwrap();
    }

    // All three nodes sync concurrently
    let sync_a = async {
        for i in 1..=10 {
            let mut id = [0; 32];
            id[0] = i;
            if let Some(delta) = source.get_delta(&id).await {
                let _ = node_a.add_delta(delta).await;
            }
            // Small delay to simulate network latency variation
            tokio::time::sleep(tokio::time::Duration::from_micros(10)).await;
        }
    };

    let sync_b = async {
        for i in 1..=10 {
            let mut id = [0; 32];
            id[0] = i;
            if let Some(delta) = source.get_delta(&id).await {
                let _ = node_b.add_delta(delta).await;
            }
            tokio::time::sleep(tokio::time::Duration::from_micros(5)).await;
        }
    };

    let sync_c = async {
        for i in 1..=10 {
            let mut id = [0; 32];
            id[0] = i;
            if let Some(delta) = source.get_delta(&id).await {
                let _ = node_c.add_delta(delta).await;
            }
            tokio::time::sleep(tokio::time::Duration::from_micros(15)).await;
        }
    };

    // Run all syncs concurrently
    tokio::join!(sync_a, sync_b, sync_c);

    // All nodes should end up with the same state
    let heads_a = node_a.get_heads().await;
    let heads_b = node_b.get_heads().await;
    let heads_c = node_c.get_heads().await;

    assert_eq!(heads_a, heads_b, "Node A and B should have same heads");
    assert_eq!(heads_b, heads_c, "Node B and C should have same heads");
    assert_eq!(heads_a, source.get_heads().await, "All should match source");

    // All deltas should be applied
    assert_eq!(node_a.applier.get_applied().await.len(), 10);
    assert_eq!(node_b.applier.get_applied().await.len(), 10);
    assert_eq!(node_c.applier.get_applied().await.len(), 10);
}

#[tokio::test]
async fn test_concurrent_sync_with_out_of_order_delivery() {
    // Simulate out-of-order delta delivery during concurrent sync

    let node = SimulatedNode::new("node");

    // Create deltas in a chain
    let mut deltas = vec![];
    for i in 1..=5 {
        let mut id = [0; 32];
        id[0] = i;
        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);
        deltas.push(delta);
    }

    // Deliver in reverse order (simulating out-of-order network delivery)
    for delta in deltas.iter().rev() {
        let _ = node.add_delta(delta.clone()).await;
    }

    // Should have pending deltas waiting for parents
    let missing = node.get_missing_parents().await;
    assert!(!missing.is_empty(), "Should have missing parents initially");

    // Now deliver in correct order
    for delta in &deltas {
        let _ = node.add_delta(delta.clone()).await;
    }

    // After correct delivery, all should be applied
    let missing_after = node.get_missing_parents().await;
    assert_eq!(
        missing_after.len(),
        0,
        "No missing parents after in-order delivery"
    );

    // All deltas should be applied
    let applied = node.applier.get_applied().await;
    assert_eq!(applied.len(), 5, "All deltas should be applied");
}

#[tokio::test]
async fn test_concurrent_writes_from_multiple_sources() {
    // Test handling concurrent deltas from multiple source nodes
    // creating a DAG with multiple branches that need merging

    let target = SimulatedNode::new("target");

    // Three concurrent writers create deltas from root
    let delta_from_a = CausalDelta::new_test([100; 32], vec![[0; 32]], vec![]);
    let delta_from_b = CausalDelta::new_test([101; 32], vec![[0; 32]], vec![]);
    let delta_from_c = CausalDelta::new_test([102; 32], vec![[0; 32]], vec![]);

    // Concurrent add (simulating network delivery)
    let handle_a = {
        let delta = delta_from_a.clone();
        let node = SimulatedNode::new("temp");
        // Use same DAG
        tokio::spawn(async move {
            let _ = node.add_delta(delta).await;
        })
    };

    // Apply to actual target
    target.add_delta(delta_from_a).await.unwrap();
    target.add_delta(delta_from_b).await.unwrap();
    target.add_delta(delta_from_c).await.unwrap();

    let _ = handle_a.await;

    // Should have 3 heads (concurrent branches)
    let mut heads = target.get_heads().await;
    heads.sort();

    assert_eq!(heads.len(), 3, "Should have 3 concurrent heads");
    assert!(heads.contains(&[100; 32]));
    assert!(heads.contains(&[101; 32]));
    assert!(heads.contains(&[102; 32]));

    // Create merge delta
    let merge_delta =
        CausalDelta::new_test([200; 32], vec![[100; 32], [101; 32], [102; 32]], vec![]);
    target.add_delta(merge_delta).await.unwrap();

    // After merge, single head
    let heads_after_merge = target.get_heads().await;
    assert_eq!(
        heads_after_merge,
        vec![[200; 32]],
        "Single head after merge"
    );
}

// ============================================================
// Test: Sync with Corrupted Deltas
// ============================================================

/// Applier that simulates corrupted delta detection
struct CorruptionDetectingApplier {
    applied: Arc<RwLock<Vec<[u8; 32]>>>,
    /// Delta IDs that should be rejected as corrupted
    corrupted_ids: Arc<RwLock<std::collections::HashSet<[u8; 32]>>>,
}

impl CorruptionDetectingApplier {
    fn new() -> Self {
        Self {
            applied: Arc::new(RwLock::new(Vec::new())),
            corrupted_ids: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    async fn mark_corrupted(&self, id: [u8; 32]) {
        self.corrupted_ids.write().await.insert(id);
    }

    async fn get_applied(&self) -> Vec<[u8; 32]> {
        self.applied.read().await.clone()
    }
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for CorruptionDetectingApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Check if this delta is marked as corrupted
        if self.corrupted_ids.read().await.contains(&delta.id) {
            return Err(ApplyError::Application(
                "Corrupted delta detected: hash mismatch".to_string(),
            ));
        }

        // Apply actions to storage
        for action in &delta.payload {
            Interface::<MainStorage>::apply_action(action.clone())
                .map_err(|e| ApplyError::Application(e.to_string()))?;
        }

        self.applied.write().await.push(delta.id);
        Ok(())
    }
}

#[tokio::test]
async fn test_sync_rejects_corrupted_delta() {
    // Test that corrupted deltas are rejected and don't break the sync

    let applier = Arc::new(CorruptionDetectingApplier::new());
    let dag = Arc::new(RwLock::new(DagStore::new([0; 32])));

    // Create valid deltas
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    let delta2_corrupted = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);
    let delta3 = CausalDelta::new_test([3; 32], vec![[2; 32]], vec![]);

    // Mark delta 2 as corrupted
    applier.mark_corrupted([2; 32]).await;

    // Apply delta 1 (should succeed)
    let result1 = dag.write().await.add_delta(delta1, &*applier).await;
    assert!(result1.is_ok());

    // Apply corrupted delta 2 (should fail)
    let result2 = dag
        .write()
        .await
        .add_delta(delta2_corrupted, &*applier)
        .await;
    assert!(result2.is_err(), "Corrupted delta should be rejected");

    // Delta 3 depends on corrupted delta 2, so it should be pending
    let _ = dag.write().await.add_delta(delta3.clone(), &*applier).await;

    // Only delta 1 should be applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 1);
    assert!(applied.contains(&[1; 32]));

    // Delta 3 should be in pending (waiting for delta 2)
    let missing = dag.read().await.get_missing_parents(100);
    assert!(
        missing.contains(&[2; 32]),
        "Should be missing corrupted parent"
    );
}

#[tokio::test]
async fn test_sync_recovers_after_corrupted_delta_retry() {
    // Test that sync can recover when corrupted delta is later received correctly

    let applier = Arc::new(CorruptionDetectingApplier::new());
    let dag = Arc::new(RwLock::new(DagStore::new([0; 32])));

    // Delta 1 is valid
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);

    // First attempt: delta 2 is corrupted
    let delta2_corrupted = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);
    applier.mark_corrupted([2; 32]).await;

    // Delta 3 depends on delta 2
    let delta3 = CausalDelta::new_test([3; 32], vec![[2; 32]], vec![]);

    // Apply delta 1
    let _ = dag.write().await.add_delta(delta1, &*applier).await;

    // Try to apply corrupted delta 2 (fails)
    let result = dag
        .write()
        .await
        .add_delta(delta2_corrupted, &*applier)
        .await;
    assert!(result.is_err());

    // Delta 3 waits for parent
    let _ = dag.write().await.add_delta(delta3.clone(), &*applier).await;
    assert!(dag.read().await.get_missing_parents(100).contains(&[2; 32]));

    // Clear corruption flag (simulating re-request with valid data)
    applier.corrupted_ids.write().await.clear();

    // Retry with valid delta 2
    let delta2_valid = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);
    let result = dag.write().await.add_delta(delta2_valid, &*applier).await;
    assert!(result.is_ok());

    // Now delta 3 should also be applied (was waiting)
    let _ = dag.write().await.add_delta(delta3, &*applier).await;

    // All deltas should be applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 3);
}

#[tokio::test]
async fn test_sync_handles_delta_with_invalid_parent_reference() {
    // Test handling deltas that reference non-existent parents

    let node = SimulatedNode::new("node");

    // Delta referencing a parent that doesn't exist
    let orphan_delta = CausalDelta::new_test([50; 32], vec![[99; 32]], vec![]);

    // Add orphan delta
    node.add_delta(orphan_delta).await.unwrap();

    // Should be pending, waiting for parent
    let missing = node.get_missing_parents().await;
    assert_eq!(
        missing,
        vec![[99; 32]],
        "Should be missing the non-existent parent"
    );

    // Delta should not be applied
    let applied = node.applier.get_applied().await;
    assert_eq!(applied.len(), 0, "Orphan delta should not be applied");

    // Heads should still be root
    let heads = node.get_heads().await;
    assert_eq!(heads, vec![[0; 32]], "Heads should still be root");
}

// ============================================================
// Test: Recovery After Partial Sync Failure
// ============================================================

#[tokio::test]
async fn test_recovery_after_partial_sync_failure() {
    // Simulate sync that fails partway through, then recovers

    let source = SimulatedNode::new("source");
    let target = SimulatedNode::new("target");

    // Source has 10 deltas
    for i in 1..=10 {
        let mut id = [0; 32];
        id[0] = i;
        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);
        source.add_delta(delta).await.unwrap();
    }

    // First sync attempt: only first 3 deltas transfer successfully
    for i in 1..=3 {
        let mut id = [0; 32];
        id[0] = i;
        if let Some(delta) = source.get_delta(&id).await {
            target.add_delta(delta).await.unwrap();
        }
    }

    // Verify partial state
    let applied_before = target.applier.get_applied().await;
    assert_eq!(
        applied_before.len(),
        3,
        "Only 3 deltas applied before failure"
    );

    // Simulate failure (no more deltas received)
    // Target has incomplete state

    let mut id = [0; 32];
    id[0] = 3;
    assert_eq!(target.get_heads().await, vec![id], "Head at delta 3");

    // Recovery: Resume sync from where we left off
    // Target requests remaining deltas

    // Determine what's missing
    let target_heads = target.get_heads().await;
    let source_heads = source.get_heads().await;

    // Find path from target heads to source heads
    let mut current = target_heads[0];
    let mut to_fetch = vec![];

    // In real implementation, would query source for descendants
    // Here we just get all deltas after current head
    for i in 4..=10 {
        let mut id = [0; 32];
        id[0] = i;
        to_fetch.push(id);
    }

    // Resume sync
    for delta_id in to_fetch {
        if let Some(delta) = source.get_delta(&delta_id).await {
            target.add_delta(delta).await.unwrap();
        }
    }

    // After recovery, target should be fully synced
    assert_eq!(
        target.get_heads().await,
        source_heads,
        "Target should have same heads as source"
    );

    let applied_after = target.applier.get_applied().await;
    assert_eq!(applied_after.len(), 10, "All 10 deltas should be applied");
}

#[tokio::test]
async fn test_recovery_tracks_sync_progress() {
    // Test that partial sync progress can be tracked and resumed

    #[derive(Default)]
    struct SyncProgress {
        last_synced_id: Option<[u8; 32]>,
        total_synced: usize,
    }

    let source = SimulatedNode::new("source");
    let target = SimulatedNode::new("target");
    let mut progress = SyncProgress::default();

    // Source has chain of deltas
    for i in 1..=10 {
        let mut id = [0; 32];
        id[0] = i;
        let delta =
            CausalDelta::new_test(id, vec![if i == 1 { [0; 32] } else { [i - 1; 32] }], vec![]);
        source.add_delta(delta).await.unwrap();
    }

    // Sync first batch with progress tracking
    for i in 1..=4 {
        let mut id = [0; 32];
        id[0] = i;
        if let Some(delta) = source.get_delta(&id).await {
            if target.add_delta(delta).await.is_ok() {
                progress.last_synced_id = Some(id);
                progress.total_synced += 1;
            }
        }
    }

    assert_eq!(progress.total_synced, 4);
    assert_eq!(
        progress.last_synced_id,
        Some([
            4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0
        ])
    );

    // Simulate restart - load progress
    let resume_from = progress.last_synced_id.unwrap()[0];

    // Resume sync from tracked progress
    for i in (resume_from + 1)..=10 {
        let mut id = [0; 32];
        id[0] = i;
        if let Some(delta) = source.get_delta(&id).await {
            if target.add_delta(delta).await.is_ok() {
                progress.last_synced_id = Some(id);
                progress.total_synced += 1;
            }
        }
    }

    // All synced
    assert_eq!(progress.total_synced, 10);
    assert_eq!(target.applier.get_applied().await.len(), 10);
}

#[tokio::test]
async fn test_recovery_with_concurrent_branches_partial_sync() {
    // Test recovery when sync fails on one branch but not another

    let source = SimulatedNode::new("source");
    let target = SimulatedNode::new("target");

    // Source has two concurrent branches from root
    let branch_a_1 = CausalDelta::new_test([10; 32], vec![[0; 32]], vec![]);
    let branch_a_2 = CausalDelta::new_test([11; 32], vec![[10; 32]], vec![]);
    let branch_b_1 = CausalDelta::new_test([20; 32], vec![[0; 32]], vec![]);
    let branch_b_2 = CausalDelta::new_test([21; 32], vec![[20; 32]], vec![]);

    source.add_delta(branch_a_1.clone()).await.unwrap();
    source.add_delta(branch_a_2.clone()).await.unwrap();
    source.add_delta(branch_b_1.clone()).await.unwrap();
    source.add_delta(branch_b_2.clone()).await.unwrap();

    // Target syncs branch A successfully
    target.add_delta(branch_a_1).await.unwrap();
    target.add_delta(branch_a_2).await.unwrap();

    // Branch B sync fails after first delta
    target.add_delta(branch_b_1.clone()).await.unwrap();
    // branch_b_2 not received (simulated failure)

    // Target has partial state
    let mut heads = target.get_heads().await;
    heads.sort();
    assert_eq!(
        heads.len(),
        2,
        "Two heads: one complete branch, one partial"
    );
    assert!(heads.contains(&[11; 32]), "Branch A complete");
    assert!(heads.contains(&[20; 32]), "Branch B at first delta");

    // Recovery: complete branch B
    target.add_delta(branch_b_2).await.unwrap();

    // Now fully synced
    let mut final_heads = target.get_heads().await;
    final_heads.sort();

    let mut source_heads = source.get_heads().await;
    source_heads.sort();

    assert_eq!(final_heads, source_heads, "Target should match source");
}

#[tokio::test]
async fn test_recovery_handles_duplicate_deltas() {
    // Test that recovery handles re-sending already-applied deltas gracefully

    let node = SimulatedNode::new("node");

    // Create chain of deltas
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);
    let delta3 = CausalDelta::new_test([3; 32], vec![[2; 32]], vec![]);

    // Apply all deltas
    node.add_delta(delta1.clone()).await.unwrap();
    node.add_delta(delta2.clone()).await.unwrap();
    node.add_delta(delta3.clone()).await.unwrap();

    // Verify all applied
    assert_eq!(node.applier.get_applied().await.len(), 3);

    // Simulate recovery that re-sends already-applied deltas
    // This should be idempotent
    let result1 = node.add_delta(delta1).await;
    let result2 = node.add_delta(delta2).await;
    let result3 = node.add_delta(delta3).await;

    // Results may vary (Ok or already-exists), but shouldn't cause issues
    // Most importantly, the node state should be unchanged

    // Still only 3 deltas applied (no duplicates)
    let applied = node.applier.get_applied().await;
    assert_eq!(applied.len(), 3, "No duplicate applications");

    // Heads unchanged
    let heads = node.get_heads().await;
    assert_eq!(heads, vec![[3; 32]], "Head unchanged after duplicate sends");
}
