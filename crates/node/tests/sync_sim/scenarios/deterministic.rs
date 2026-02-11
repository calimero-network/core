//! Deterministic test scenarios.
//!
//! See spec §15 - Protocol Negotiation Tests.

use calimero_primitives::crdt::CrdtType;

use crate::sync_sim::actions::EntityMetadata;
use crate::sync_sim::node::SimNode;
use crate::sync_sim::types::{DeltaId, EntityId};

/// Helper to generate entities.
pub fn generate_entities(count: usize, seed: u64) -> Vec<(EntityId, Vec<u8>, EntityMetadata)> {
    (0..count)
        .map(|i| {
            let id = EntityId::from_u64(seed * 10000 + i as u64);
            let data = format!("entity-{}-{}", seed, i).into_bytes();
            let metadata = EntityMetadata::new(CrdtType::LwwRegister, i as u64 * 100);
            (id, data, metadata)
        })
        .collect()
}

/// Generate entities forming a deep tree structure.
///
/// Keys are structured to produce `max_depth` in Merkle tree.
pub fn generate_deep_tree_entities(
    count: usize,
    depth: u32,
    seed: u64,
) -> Vec<(EntityId, Vec<u8>, EntityMetadata)> {
    (0..count)
        .map(|i| {
            let mut key = [0u8; 32];
            // Spread across tree levels
            for d in 0..depth {
                // Ensure divisor is at least 1 to prevent divide by zero
                let divisor = (count / (1_usize << d).max(1)).max(1);
                key[d as usize] = ((i / divisor) % 256) as u8;
            }
            key[24..32].copy_from_slice(&(seed + i as u64).to_le_bytes());

            let id = EntityId::from_bytes(key);
            let data = format!("deep-entity-{}-{}", seed, i).into_bytes();
            let metadata = EntityMetadata::new(CrdtType::LwwRegister, i as u64 * 100);
            (id, data, metadata)
        })
        .collect()
}

/// Generate entities forming a wide shallow tree.
pub fn generate_shallow_wide_tree(
    count: usize,
    depth: u32,
    seed: u64,
) -> Vec<(EntityId, Vec<u8>, EntityMetadata)> {
    assert!(depth <= 2, "shallow tree depth must be <= 2");

    (0..count)
        .map(|i| {
            let mut key = [0u8; 32];
            // All keys share first (32 - depth) bytes → shallow tree
            key[0] = (i / 256) as u8; // Level 1 fanout
            key[1] = (i % 256) as u8; // Level 2 fanout (if depth=2)
            key[24..32].copy_from_slice(&(seed + i as u64).to_le_bytes());

            let id = EntityId::from_bytes(key);
            let data = format!("shallow-entity-{}-{}", seed, i).into_bytes();
            let metadata = EntityMetadata::new(CrdtType::LwwRegister, i as u64 * 100);
            (id, data, metadata)
        })
        .collect()
}

/// Scenario builders for protocol negotiation testing.
///
/// Each method sets up nodes in specific states to trigger deterministic
/// protocol selection per CIP §2.3 rules.
pub struct Scenario;

impl Scenario {
    /// Rule 1: Same root hash → None.
    ///
    /// Creates two nodes with identical state.
    pub fn force_none() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Identical state
        let entities = generate_entities(100, 1);
        for (id, data, metadata) in &entities {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        assert_eq!(a.root_hash(), b.root_hash(), "root hashes should match");
        (a, b)
    }

    /// Rule 2: Fresh node → Snapshot.
    ///
    /// Creates a fresh node and an initialized source.
    pub fn force_snapshot() -> (SimNode, SimNode) {
        let fresh = SimNode::new("fresh"); // Empty, has_state = false
        let mut source = SimNode::new("source");

        for (id, data, metadata) in generate_entities(100, 2) {
            source.insert_entity_with_metadata(id, data, metadata);
        }

        assert!(!fresh.has_any_state(), "fresh node should have no state");
        assert!(source.has_any_state(), "source should have state");
        (fresh, source)
    }

    /// Rule 3: High divergence (>50%) → HashComparison.
    ///
    /// Creates two nodes with significant difference.
    pub fn force_hash_high_divergence() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // A has 40 entities (seed 1)
        for (id, data, metadata) in generate_entities(40, 1) {
            a.insert_entity_with_metadata(id, data, metadata);
        }

        // B has 100 entities (seed 2) → 60% divergence
        for (id, data, metadata) in generate_entities(100, 2) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        assert!(a.has_any_state());
        assert!(b.has_any_state());
        // Divergence should be >50%
        (a, b)
    }

    /// Rule 4: Deep tree + low divergence → SubtreePrefetch.
    ///
    /// Creates deep tree structures with small difference.
    pub fn force_subtree_prefetch() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Shared base (80 entities)
        let shared = generate_deep_tree_entities(80, 5, 1);
        for (id, data, metadata) in &shared {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        // A has 5 extra, B has 15 extra → 15% divergence
        for (id, data, metadata) in generate_deep_tree_entities(5, 5, 2) {
            a.insert_entity_with_metadata(id, data, metadata);
        }
        for (id, data, metadata) in generate_deep_tree_entities(15, 5, 3) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        (a, b)
    }

    /// Rule 5: Large tree + small diff → BloomFilter.
    ///
    /// Creates nodes with mostly shared state and small difference.
    pub fn force_bloom_filter() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Shared base (95 entities)
        let shared = generate_entities(95, 1);
        for (id, data, metadata) in &shared {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        // B has 5 extra → 5% divergence, >50 entities
        for (id, data, metadata) in generate_entities(5, 2) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        assert!(b.entity_count() > 50);
        (a, b)
    }

    /// Rule 6: Wide shallow tree → LevelWise.
    ///
    /// Creates shallow tree structures.
    pub fn force_levelwise() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Shallow trees (depth ≤ 2) with many children per level
        for (id, data, metadata) in generate_shallow_wide_tree(36, 2, 1) {
            a.insert_entity_with_metadata(id, data, metadata);
        }
        for (id, data, metadata) in generate_shallow_wide_tree(40, 2, 2) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        (a, b)
    }

    /// DeltaSync: Small gap in DAG.
    ///
    /// Creates nodes with identical state but different DAG heads.
    pub fn force_delta_sync() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Both start with same state
        let base = generate_entities(100, 1);
        for (id, data, metadata) in &base {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        // Sync DAG heads
        let base_head = DeltaId::from_bytes([1; 32]);
        a.dag_heads = vec![base_head];
        b.dag_heads = vec![base_head];

        // B gets 2 new deltas (small gap)
        for (id, data, metadata) in generate_entities(2, 2) {
            b.insert_entity_with_metadata(id, data, metadata);
        }
        // Update B's DAG head
        b.dag_heads = vec![DeltaId::from_bytes([2; 32])];

        // DAG heads differ, but small divergence
        assert_ne!(a.dag_heads(), b.dag_heads());
        (a, b)
    }

    /// Both nodes initialized with partial overlap.
    pub fn partial_overlap() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Shared entities
        let shared = generate_entities(50, 1);
        for (id, data, metadata) in &shared {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        // A-only entities
        for (id, data, metadata) in generate_entities(25, 2) {
            a.insert_entity_with_metadata(id, data, metadata);
        }

        // B-only entities
        for (id, data, metadata) in generate_entities(25, 3) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        (a, b)
    }

    /// Both nodes initialized (for invariant I5 testing).
    pub fn both_initialized() -> (SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        for (id, data, metadata) in generate_entities(50, 1) {
            a.insert_entity_with_metadata(id, data, metadata);
        }

        for (id, data, metadata) in generate_entities(50, 2) {
            b.insert_entity_with_metadata(id, data, metadata);
        }

        assert!(a.has_any_state());
        assert!(b.has_any_state());
        (a, b)
    }

    /// Fresh node and initialized node (for snapshot testing).
    pub fn fresh_and_initialized() -> (SimNode, SimNode) {
        Self::force_snapshot()
    }

    /// Three nodes: A and B synced, C diverged.
    pub fn three_nodes_one_diverged() -> (SimNode, SimNode, SimNode) {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");
        let mut c = SimNode::new("c");

        // A and B have same state
        let shared = generate_entities(50, 1);
        for (id, data, metadata) in &shared {
            a.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
            b.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
        }

        // C has different state
        for (id, data, metadata) in generate_entities(50, 2) {
            c.insert_entity_with_metadata(id, data, metadata);
        }

        assert_eq!(a.root_hash(), b.root_hash());
        assert_ne!(a.root_hash(), c.root_hash());

        (a, b, c)
    }

    /// Create N nodes, all with same state.
    pub fn n_nodes_synced(n: usize) -> Vec<SimNode> {
        let entities = generate_entities(50, 1);

        (0..n)
            .map(|i| {
                let mut node = SimNode::new(format!("node-{}", i));
                for (id, data, metadata) in &entities {
                    node.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
                }
                node
            })
            .collect()
    }

    /// Create N nodes, each with different state.
    pub fn n_nodes_diverged(n: usize) -> Vec<SimNode> {
        (0..n)
            .map(|i| {
                let mut node = SimNode::new(format!("node-{}", i));
                for (id, data, metadata) in generate_entities(50, i as u64 + 1) {
                    node.insert_entity_with_metadata(id, data, metadata);
                }
                node
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_force_none() {
        let (mut a, mut b) = Scenario::force_none();
        assert_eq!(a.root_hash(), b.root_hash());
        assert!(a.has_any_state());
        assert!(b.has_any_state());
    }

    #[test]
    fn test_force_snapshot() {
        let (fresh, source) = Scenario::force_snapshot();
        assert!(!fresh.has_any_state());
        assert!(source.has_any_state());
    }

    #[test]
    fn test_force_hash_high_divergence() {
        let (a, b) = Scenario::force_hash_high_divergence();
        assert!(a.has_any_state());
        assert!(b.has_any_state());
        // Significant entity count difference
        assert!(b.entity_count() > a.entity_count() * 2);
    }

    #[test]
    fn test_partial_overlap() {
        let (a, b) = Scenario::partial_overlap();
        assert_eq!(a.entity_count(), 75); // 50 shared + 25 unique
        assert_eq!(b.entity_count(), 75); // 50 shared + 25 unique
    }

    #[test]
    fn test_n_nodes_synced() {
        let mut nodes = Scenario::n_nodes_synced(5);
        assert_eq!(nodes.len(), 5);

        let hashes: Vec<_> = nodes.iter_mut().map(|n| n.storage.digest()).collect();
        assert!(hashes.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn test_n_nodes_diverged() {
        let mut nodes = Scenario::n_nodes_diverged(5);
        assert_eq!(nodes.len(), 5);

        let hashes: Vec<_> = nodes.iter_mut().map(|n| n.storage.digest()).collect();
        // All different
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(hashes[i], hashes[j]);
            }
        }
    }
}
