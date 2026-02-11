//! Test assertion macros and helpers.

use crate::sync_sim::node::SimNode;
use crate::sync_sim::types::StateDigest;

/// Check if two nodes have converged (same state digest).
pub fn nodes_converged(a: &mut SimNode, b: &mut SimNode) -> bool {
    a.state_digest() == b.state_digest()
}

/// Check if all nodes have the same state digest.
pub fn all_converged(nodes: &mut [SimNode]) -> bool {
    if nodes.is_empty() {
        return true;
    }

    let first = nodes[0].state_digest();
    nodes.iter_mut().all(|n| n.state_digest() == first)
}

/// Get the majority digest from nodes.
pub fn majority_digest(nodes: &mut [SimNode]) -> Option<StateDigest> {
    use std::collections::HashMap;

    let mut counts: HashMap<StateDigest, usize> = HashMap::new();
    for node in nodes.iter_mut() {
        *counts.entry(node.state_digest()).or_default() += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(digest, _)| digest)
}

/// Compute divergence percentage between two nodes.
///
/// Entities are considered "shared" only if they have the same ID AND
/// the same content (data + metadata). This catches cases where both nodes
/// have an entity with the same ID but different values.
pub fn divergence_percentage(a: &SimNode, b: &SimNode) -> f64 {
    let a_count = a.entity_count();
    let b_count = b.entity_count();

    if a_count == 0 && b_count == 0 {
        return 0.0;
    }

    // Count truly shared entities (same ID AND same content)
    let mut shared = 0;
    for entity_a in a.storage.iter() {
        if let Some(entity_b) = b.storage.get(&entity_a.id) {
            // Only count as shared if data and all metadata fields match
            if entity_a.data == entity_b.data && entity_a.metadata == entity_b.metadata {
                shared += 1;
            }
        }
    }

    let total = a_count + b_count - shared;
    if total == 0 {
        return 0.0;
    }

    let different = total - shared;
    different as f64 / total as f64
}

/// Assert that nodes have converged.
///
/// Note: `macro_rules!` macros in a module are automatically available
/// to sibling modules without `#[macro_export]` when accessed via the parent.
macro_rules! assert_converged {
    ($($node:expr),+ $(,)?) => {{
        let nodes: &mut [&mut $crate::sync_sim::node::SimNode] = &mut [$(&mut $node),+];
        let digests: Vec<_> = nodes.iter_mut().map(|n| n.state_digest()).collect();

        if !digests.windows(2).all(|w| w[0] == w[1]) {
            panic!(
                "Nodes not converged!\nDigests:\n{}",
                nodes.iter()
                    .zip(digests.iter())
                    .map(|(n, d)| format!("  {}: {:?}", n.id(), d))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }};
}

/// Assert that nodes have NOT converged.
macro_rules! assert_not_converged {
    ($($node:expr),+ $(,)?) => {{
        let nodes: &mut [&mut $crate::sync_sim::node::SimNode] = &mut [$(&mut $node),+];
        let digests: Vec<_> = nodes.iter_mut().map(|n| n.state_digest()).collect();

        if digests.windows(2).all(|w| w[0] == w[1]) {
            panic!("Nodes unexpectedly converged with digest: {:?}", digests[0]);
        }
    }};
}

/// Assert that a node has specific entity count.
macro_rules! assert_entity_count {
    ($node:expr, $count:expr) => {{
        let actual = $node.entity_count();
        let expected = $count;
        if actual != expected {
            panic!(
                "Node {} entity count mismatch: expected {}, got {}",
                $node.id(),
                expected,
                actual
            );
        }
    }};
}

/// Assert that a node has an entity.
macro_rules! assert_has_entity {
    ($node:expr, $id:expr) => {{
        if !$node.has_entity(&$id) {
            panic!("Node {} missing entity {:?}", $node.id(), $id);
        }
    }};
}

/// Assert that a node does not have an entity.
macro_rules! assert_no_entity {
    ($node:expr, $id:expr) => {{
        if $node.has_entity(&$id) {
            panic!("Node {} unexpectedly has entity {:?}", $node.id(), $id);
        }
    }};
}

/// Assert that a node is in idle sync state.
macro_rules! assert_idle {
    ($node:expr) => {{
        if !$node.sync_state.is_idle() {
            panic!(
                "Node {} not idle, state: {:?}",
                $node.id(),
                $node.sync_state
            );
        }
    }};
}

/// Assert that a node has empty delta buffer.
macro_rules! assert_buffer_empty {
    ($node:expr) => {{
        if !$node.delta_buffer.is_empty() {
            panic!(
                "Node {} buffer not empty, size: {}",
                $node.id(),
                $node.buffer_size()
            );
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_sim::types::EntityId;
    use calimero_primitives::crdt::CrdtType;

    #[test]
    fn test_nodes_converged() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Empty nodes are converged
        assert!(nodes_converged(&mut a, &mut b));

        // Add same entity to both
        let id = EntityId::from_u64(1);
        a.insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);
        b.insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);

        assert!(nodes_converged(&mut a, &mut b));

        // Add different entity to one
        a.insert_entity(EntityId::from_u64(2), vec![4, 5, 6], CrdtType::LwwRegister);

        assert!(!nodes_converged(&mut a, &mut b));
    }

    #[test]
    fn test_all_converged() {
        let mut nodes = vec![SimNode::new("a"), SimNode::new("b"), SimNode::new("c")];

        // Empty nodes are converged
        assert!(all_converged(&mut nodes));

        // Add same entity to all
        let id = EntityId::from_u64(1);
        for node in &mut nodes {
            node.insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);
        }

        assert!(all_converged(&mut nodes));

        // Modify one
        nodes[1].insert_entity(EntityId::from_u64(2), vec![4], CrdtType::LwwRegister);

        assert!(!all_converged(&mut nodes));
    }

    #[test]
    fn test_divergence_percentage() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Empty nodes have 0% divergence
        assert_eq!(divergence_percentage(&a, &b), 0.0);

        // Same entities
        let id = EntityId::from_u64(1);
        a.insert_entity(id, vec![1], CrdtType::LwwRegister);
        b.insert_entity(id, vec![1], CrdtType::LwwRegister);

        assert_eq!(divergence_percentage(&a, &b), 0.0);

        // Add unique entity to A
        a.insert_entity(EntityId::from_u64(2), vec![2], CrdtType::LwwRegister);

        // 1 shared, 1 unique = 2 total, 1 different = 50%
        let div = divergence_percentage(&a, &b);
        assert!((div - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_divergence_content_aware() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Same ID but different content should be considered divergent
        let id = EntityId::from_u64(1);
        a.insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);
        b.insert_entity(id, vec![4, 5, 6], CrdtType::LwwRegister); // Different data!

        // Both have 1 entity, but they conflict
        // total = 1 + 1 - 0 (shared) = 2, different = 2
        // divergence = 2/2 = 100%
        let div = divergence_percentage(&a, &b);
        assert!(
            (div - 1.0).abs() < 0.001,
            "Expected 100% divergence for conflicting content, got {}",
            div
        );
    }

    #[test]
    fn test_assert_converged_macro() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Should pass
        assert_converged!(a, b);

        // Add same entity
        let id = EntityId::from_u64(1);
        a.insert_entity(id, vec![1], CrdtType::LwwRegister);
        b.insert_entity(id, vec![1], CrdtType::LwwRegister);

        assert_converged!(a, b);
    }

    #[test]
    #[should_panic(expected = "Nodes not converged")]
    fn test_assert_converged_macro_fails() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        a.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);

        assert_converged!(a, b);
    }

    #[test]
    fn test_assert_not_converged_macro() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        a.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);

        assert_not_converged!(a, b);
    }

    #[test]
    fn test_assert_entity_count_macro() {
        let mut a = SimNode::new("a");
        assert_entity_count!(a, 0);

        a.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);
        assert_entity_count!(a, 1);
    }

    #[test]
    fn test_assert_has_entity_macro() {
        let mut a = SimNode::new("a");
        let id = EntityId::from_u64(1);

        a.insert_entity(id, vec![1], CrdtType::LwwRegister);
        assert_has_entity!(a, id);
    }

    #[test]
    fn test_assert_idle_macro() {
        let a = SimNode::new("a");
        assert_idle!(a);
    }

    #[test]
    fn test_assert_buffer_empty_macro() {
        let a = SimNode::new("a");
        assert_buffer_empty!(a);
    }
}
