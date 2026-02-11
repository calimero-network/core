//! LevelWise sync types (CIP Appendix B - Protocol Selection Matrix).
//!
//! Types for level-by-level breadth-first synchronization, optimized for wide,
//! shallow trees with scattered changes.
//!
//! # When to Use
//!
//! - `max_depth <= 2` (shallow trees)
//! - `avg_children_per_level > 10` (wide trees)
//! - Changes scattered across siblings
//!
//! # Protocol Flow
//!
//! ```text
//! Initiator                          Responder
//!     │                                   │
//!     │ ──── LevelWiseRequest ──────────► │
//!     │      { level: 0 }                 │
//!     │                                   │
//!     │ ◄──── LevelWiseResponse ───────── │
//!     │      { nodes at level 0 }         │
//!     │                                   │
//!     │ (Compare hashes, identify diff)   │
//!     │                                   │
//!     │ ──── LevelWiseRequest ──────────► │
//!     │      { level: 1, parent_ids }     │
//!     │                                   │
//! ```
//!
//! # Trade-offs
//!
//! | Aspect        | HashComparison     | LevelWise            |
//! |---------------|--------------------|-----------------------|
//! | Round trips   | O(depth)           | O(depth)              |
//! | Messages/round| 1                  | Batched by level      |
//! | Best for      | Deep trees         | Wide shallow trees    |
//!
//! # Validation
//!
//! All types have `is_valid()` methods that should be called after deserializing
//! from untrusted sources to prevent resource exhaustion attacks.

use std::collections::{HashMap, HashSet};

use borsh::{BorshDeserialize, BorshSerialize};

use super::hash_comparison::TreeLeafData;

// =============================================================================
// Constants
// =============================================================================

/// Maximum depth for level-wise sync traversal.
///
/// LevelWise is designed for shallow trees (depth <= 2), but we allow up to
/// this limit for flexibility. Aligned with `hash_comparison::MAX_TREE_DEPTH`.
pub const MAX_LEVELWISE_DEPTH: usize = 64;

/// Maximum number of parent IDs in a single request.
///
/// Limits the size of `LevelWiseRequest::parent_ids` to prevent DoS attacks
/// from malicious peers sending oversized requests.
pub const MAX_PARENTS_PER_REQUEST: usize = 1000;

/// Maximum number of nodes in a single response.
///
/// Limits the size of `LevelWiseResponse::nodes` to prevent memory exhaustion
/// from malicious peers sending oversized responses.
pub const MAX_NODES_PER_LEVEL: usize = 10_000;

// =============================================================================
// LevelWise Request/Response
// =============================================================================

/// Request for level-wise breadth-first synchronization.
///
/// Processes the tree level-by-level, comparing hashes at each level.
/// Efficient for wide, shallow trees with scattered changes.
///
/// Use when:
/// - max_depth <= 2
/// - Wide trees with many children at each level
/// - Changes scattered across siblings
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LevelWiseRequest {
    /// Level to request (0 = root's children, 1 = grandchildren, etc.).
    pub level: usize,

    /// Parent IDs to fetch children for (None = fetch all at this level).
    /// Used to narrow down which subtrees to explore.
    ///
    /// Limited to MAX_PARENTS_PER_REQUEST entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub parent_ids: Option<Vec<[u8; 32]>>,
}

impl LevelWiseRequest {
    /// Request all nodes at a given level.
    #[must_use]
    pub fn at_level(level: usize) -> Self {
        Self {
            level,
            parent_ids: None,
        }
    }

    /// Request children of specific parents at a given level.
    #[must_use]
    pub fn for_parents(level: usize, parent_ids: Vec<[u8; 32]>) -> Self {
        Self {
            level,
            parent_ids: Some(parent_ids),
        }
    }

    /// Check if this requests all nodes at the level.
    #[must_use]
    pub fn is_full_level(&self) -> bool {
        self.parent_ids.is_none()
    }

    /// Get number of parents being queried (None if full level).
    #[must_use]
    pub fn parent_count(&self) -> Option<usize> {
        self.parent_ids.as_ref().map(|p| p.len())
    }

    /// Check if request is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// resource exhaustion attacks.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check level limit
        if self.level > MAX_LEVELWISE_DEPTH {
            return false;
        }

        // Check parent_ids count limit
        if let Some(ref parents) = self.parent_ids {
            if parents.len() > MAX_PARENTS_PER_REQUEST {
                return false;
            }
        }

        true
    }
}

/// Response containing nodes at a specific level.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LevelWiseResponse {
    /// Level these nodes are at.
    pub level: usize,

    /// Nodes at this level.
    ///
    /// Limited to MAX_NODES_PER_LEVEL entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub nodes: Vec<LevelNode>,

    /// Whether there are more levels below this one.
    pub has_more_levels: bool,
}

impl LevelWiseResponse {
    /// Create a response with nodes.
    #[must_use]
    pub fn new(level: usize, nodes: Vec<LevelNode>, has_more_levels: bool) -> Self {
        Self {
            level,
            nodes,
            has_more_levels,
        }
    }

    /// Create an empty response (no nodes at this level).
    #[must_use]
    pub fn empty(level: usize) -> Self {
        Self {
            level,
            nodes: vec![],
            has_more_levels: false,
        }
    }

    /// Number of nodes at this level.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Check if this level is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Get an iterator over leaf nodes at this level.
    pub fn leaves(&self) -> impl Iterator<Item = &LevelNode> {
        self.nodes.iter().filter(|n| n.is_leaf())
    }

    /// Get an iterator over internal nodes at this level.
    pub fn internal_nodes(&self) -> impl Iterator<Item = &LevelNode> {
        self.nodes.iter().filter(|n| n.is_internal())
    }

    /// Get IDs of all internal nodes (for next level request).
    #[must_use]
    pub fn internal_node_ids(&self) -> Vec<[u8; 32]> {
        self.nodes
            .iter()
            .filter(|n| n.is_internal())
            .map(|n| n.id)
            .collect()
    }

    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// resource exhaustion attacks. Validates both response size and all
    /// contained nodes.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check level limit
        if self.level > MAX_LEVELWISE_DEPTH {
            return false;
        }

        // Check nodes count limit
        if self.nodes.len() > MAX_NODES_PER_LEVEL {
            return false;
        }

        // Validate all nodes
        self.nodes.iter().all(LevelNode::is_valid)
    }
}

// =============================================================================
// LevelNode
// =============================================================================

/// A node in the level-wise traversal.
///
/// Contains enough information to compare with local state
/// and decide whether to recurse or fetch leaf data.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LevelNode {
    /// Node ID.
    pub id: [u8; 32],

    /// Merkle hash of this node.
    pub hash: [u8; 32],

    /// Parent node ID (None for root children).
    pub parent_id: Option<[u8; 32]>,

    /// Leaf data (present only for leaf nodes).
    /// Includes full data and metadata for CRDT merge.
    pub leaf_data: Option<TreeLeafData>,
}

impl LevelNode {
    /// Create an internal node.
    #[must_use]
    pub fn internal(id: [u8; 32], hash: [u8; 32], parent_id: Option<[u8; 32]>) -> Self {
        Self {
            id,
            hash,
            parent_id,
            leaf_data: None,
        }
    }

    /// Create a leaf node.
    #[must_use]
    pub fn leaf(
        id: [u8; 32],
        hash: [u8; 32],
        parent_id: Option<[u8; 32]>,
        data: TreeLeafData,
    ) -> Self {
        Self {
            id,
            hash,
            parent_id,
            leaf_data: Some(data),
        }
    }

    /// Check if this is a leaf node.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.leaf_data.is_some()
    }

    /// Check if this is an internal node.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        self.leaf_data.is_none()
    }

    /// Check if node is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Validate leaf data if present
        if let Some(ref leaf_data) = self.leaf_data {
            if !leaf_data.is_valid() {
                return false;
            }
        }

        true
    }
}

// =============================================================================
// Level Compare Result
// =============================================================================

/// Result of comparing nodes at a level.
#[derive(Clone, Debug, Default)]
pub struct LevelCompareResult {
    /// Nodes that match (same hash).
    pub matching: Vec<[u8; 32]>,

    /// Nodes that differ (different hash) - need to recurse or fetch.
    pub differing: Vec<[u8; 32]>,

    /// Nodes missing locally - need to fetch.
    pub local_missing: Vec<[u8; 32]>,

    /// Nodes missing remotely - nothing to do.
    pub remote_missing: Vec<[u8; 32]>,
}

impl LevelCompareResult {
    /// Check if any sync work is needed.
    #[must_use]
    pub fn needs_sync(&self) -> bool {
        !self.differing.is_empty() || !self.local_missing.is_empty()
    }

    /// Get all node IDs that need further processing.
    #[must_use]
    pub fn nodes_to_process(&self) -> Vec<[u8; 32]> {
        let mut result = self.differing.clone();
        result.extend(self.local_missing.iter().copied());
        result
    }

    /// Total number of nodes compared.
    #[must_use]
    pub fn total_compared(&self) -> usize {
        self.matching.len()
            + self.differing.len()
            + self.local_missing.len()
            + self.remote_missing.len()
    }
}

// =============================================================================
// Compare Function
// =============================================================================

/// Compare local and remote nodes at a level.
///
/// Takes a map of local node hashes and the remote response,
/// and categorizes each node.
#[must_use]
pub fn compare_level_nodes(
    local_hashes: &HashMap<[u8; 32], [u8; 32]>,
    remote: &LevelWiseResponse,
) -> LevelCompareResult {
    let mut result = LevelCompareResult::default();

    // Check each remote node against local
    for node in &remote.nodes {
        match local_hashes.get(&node.id) {
            Some(local_hash) if *local_hash == node.hash => {
                result.matching.push(node.id);
            }
            Some(_) => {
                result.differing.push(node.id);
            }
            None => {
                result.local_missing.push(node.id);
            }
        }
    }

    // Find nodes that exist locally but not in remote response
    let remote_ids: HashSet<_> = remote.nodes.iter().map(|n| n.id).collect();
    for local_id in local_hashes.keys() {
        if !remote_ids.contains(local_id) {
            result.remote_missing.push(*local_id);
        }
    }

    result
}

// =============================================================================
// Heuristic Function
// =============================================================================

/// Check if LevelWise sync is appropriate for a tree.
///
/// Returns true if LevelWise is likely more efficient than HashComparison.
#[must_use]
pub fn should_use_levelwise(tree_depth: usize, avg_children_per_level: usize) -> bool {
    // LevelWise is better for wide, shallow trees
    tree_depth <= 2 && avg_children_per_level > 10
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::hash_comparison::{CrdtType, LeafMetadata, MAX_LEAF_VALUE_SIZE};

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn make_leaf_data(key: u8, value: Vec<u8>) -> TreeLeafData {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [key; 32]);
        TreeLeafData::new([key; 32], value, metadata)
    }

    // =========================================================================
    // LevelWiseRequest Tests
    // =========================================================================

    #[test]
    fn test_levelwise_request_at_level() {
        let request = LevelWiseRequest::at_level(2);

        assert_eq!(request.level, 2);
        assert!(request.is_full_level());
        assert!(request.parent_count().is_none());
        assert!(request.is_valid());
    }

    #[test]
    fn test_levelwise_request_for_parents() {
        let parents = vec![[1u8; 32], [2u8; 32]];
        let request = LevelWiseRequest::for_parents(1, parents.clone());

        assert_eq!(request.level, 1);
        assert!(!request.is_full_level());
        assert_eq!(request.parent_count(), Some(2));
        assert_eq!(request.parent_ids, Some(parents));
        assert!(request.is_valid());
    }

    #[test]
    fn test_levelwise_request_roundtrip() {
        let request = LevelWiseRequest::for_parents(3, vec![[1u8; 32]]);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: LevelWiseRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    #[test]
    fn test_levelwise_request_validation() {
        // Valid request at level limit
        let at_limit = LevelWiseRequest::at_level(MAX_LEVELWISE_DEPTH);
        assert!(at_limit.is_valid());

        // Invalid request over level limit
        let over_level = LevelWiseRequest::at_level(MAX_LEVELWISE_DEPTH + 1);
        assert!(!over_level.is_valid());

        // Valid request at parent limit
        let parents: Vec<[u8; 32]> = (0..MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let at_parent_limit = LevelWiseRequest::for_parents(0, parents);
        assert!(at_parent_limit.is_valid());

        // Invalid request over parent limit
        let parents: Vec<[u8; 32]> = (0..=MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let over_parent_limit = LevelWiseRequest::for_parents(0, parents);
        assert!(!over_parent_limit.is_valid());
    }

    // =========================================================================
    // LevelNode Tests
    // =========================================================================

    #[test]
    fn test_level_node_internal() {
        let node = LevelNode::internal([1; 32], [2; 32], Some([0; 32]));

        assert!(node.is_internal());
        assert!(!node.is_leaf());
        assert_eq!(node.parent_id, Some([0; 32]));
        assert!(node.is_valid());
    }

    #[test]
    fn test_level_node_leaf() {
        let leaf_data = make_leaf_data(3, vec![1, 2, 3]);
        let node = LevelNode::leaf([1; 32], [2; 32], None, leaf_data);

        assert!(node.is_leaf());
        assert!(!node.is_internal());
        assert!(node.parent_id.is_none());
        assert!(node.is_valid());
    }

    #[test]
    fn test_level_node_roundtrip() {
        let leaf_data = make_leaf_data(4, vec![4, 5, 6]);
        let node = LevelNode::leaf([1; 32], [2; 32], Some([0; 32]), leaf_data);

        let encoded = borsh::to_vec(&node).expect("serialize");
        let decoded: LevelNode = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(node, decoded);
    }

    #[test]
    fn test_level_node_validation() {
        // Valid internal node
        let internal = LevelNode::internal([1; 32], [2; 32], None);
        assert!(internal.is_valid());

        // Valid leaf node
        let leaf_data = make_leaf_data(1, vec![1, 2, 3]);
        let valid_leaf = LevelNode::leaf([1; 32], [2; 32], None, leaf_data);
        assert!(valid_leaf.is_valid());

        // Invalid leaf node with oversized value
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let invalid_leaf_data =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_leaf = LevelNode::leaf([1; 32], [2; 32], None, invalid_leaf_data);
        assert!(!invalid_leaf.is_valid());
    }

    // =========================================================================
    // LevelWiseResponse Tests
    // =========================================================================

    #[test]
    fn test_levelwise_response_new() {
        let node1 = LevelNode::internal([1; 32], [2; 32], None);
        let node2 = LevelNode::internal([3; 32], [4; 32], None);

        let response = LevelWiseResponse::new(0, vec![node1, node2], true);

        assert_eq!(response.level, 0);
        assert_eq!(response.node_count(), 2);
        assert!(response.has_more_levels);
        assert!(!response.is_empty());
        assert!(response.is_valid());
    }

    #[test]
    fn test_levelwise_response_empty() {
        let response = LevelWiseResponse::empty(3);

        assert_eq!(response.level, 3);
        assert!(response.is_empty());
        assert!(!response.has_more_levels);
        assert!(response.is_valid());
    }

    #[test]
    fn test_levelwise_response_leaves_and_internal() {
        let internal = LevelNode::internal([1; 32], [2; 32], None);
        let leaf_data = make_leaf_data(6, vec![7, 8]);
        let leaf = LevelNode::leaf([3; 32], [4; 32], None, leaf_data);

        let response = LevelWiseResponse::new(1, vec![internal, leaf], false);

        assert_eq!(response.leaves().count(), 1);
        assert_eq!(response.internal_nodes().count(), 1);
        assert_eq!(response.internal_node_ids(), vec![[1; 32]]);
    }

    #[test]
    fn test_levelwise_response_roundtrip() {
        let node = LevelNode::internal([1; 32], [2; 32], Some([0; 32]));
        let response = LevelWiseResponse::new(2, vec![node], true);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: LevelWiseResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_levelwise_response_validation() {
        // Valid response at node limit
        let nodes: Vec<LevelNode> = (0..MAX_NODES_PER_LEVEL)
            .map(|i| LevelNode::internal([i as u8; 32], [i as u8; 32], None))
            .collect();
        let at_limit = LevelWiseResponse::new(0, nodes, false);
        assert!(at_limit.is_valid());

        // Invalid response over node limit
        let nodes: Vec<LevelNode> = (0..=MAX_NODES_PER_LEVEL)
            .map(|i| LevelNode::internal([i as u8; 32], [i as u8; 32], None))
            .collect();
        let over_limit = LevelWiseResponse::new(0, nodes, false);
        assert!(!over_limit.is_valid());

        // Invalid response over level limit
        let over_level = LevelWiseResponse::new(MAX_LEVELWISE_DEPTH + 1, vec![], false);
        assert!(!over_level.is_valid());

        // Invalid response with invalid node
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let invalid_leaf_data =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_node = LevelNode::leaf([1; 32], [2; 32], None, invalid_leaf_data);
        let response_with_invalid = LevelWiseResponse::new(0, vec![invalid_node], false);
        assert!(!response_with_invalid.is_valid());
    }

    // =========================================================================
    // LevelCompareResult Tests
    // =========================================================================

    #[test]
    fn test_level_compare_result() {
        let mut result = LevelCompareResult::default();
        result.matching.push([1; 32]);
        result.differing.push([2; 32]);
        result.local_missing.push([3; 32]);
        result.remote_missing.push([4; 32]);

        assert!(result.needs_sync());
        assert_eq!(result.total_compared(), 4);
        assert_eq!(result.nodes_to_process().len(), 2); // differing + local_missing
    }

    #[test]
    fn test_level_compare_result_no_sync() {
        let mut result = LevelCompareResult::default();
        result.matching.push([1; 32]);
        result.remote_missing.push([2; 32]);

        assert!(!result.needs_sync());
        assert!(result.nodes_to_process().is_empty());
    }

    // =========================================================================
    // compare_level_nodes Tests
    // =========================================================================

    #[test]
    fn test_compare_level_nodes() {
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]); // Same hash
        local_hashes.insert([2; 32], [20; 32]); // Different hash (local has 20, remote has 21)
        local_hashes.insert([4; 32], [40; 32]); // Only in local

        let remote_nodes = vec![
            LevelNode::internal([1; 32], [10; 32], None), // Matches
            LevelNode::internal([2; 32], [21; 32], None), // Differs
            LevelNode::internal([3; 32], [30; 32], None), // Only in remote
        ];
        let response = LevelWiseResponse::new(0, remote_nodes, true);

        let result = compare_level_nodes(&local_hashes, &response);

        assert_eq!(result.matching, vec![[1; 32]]);
        assert_eq!(result.differing, vec![[2; 32]]);
        assert_eq!(result.local_missing, vec![[3; 32]]);
        assert_eq!(result.remote_missing, vec![[4; 32]]);
    }

    #[test]
    fn test_compare_level_nodes_all_matching() {
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]);
        local_hashes.insert([2; 32], [20; 32]);

        let remote_nodes = vec![
            LevelNode::internal([1; 32], [10; 32], None),
            LevelNode::internal([2; 32], [20; 32], None),
        ];
        let response = LevelWiseResponse::new(0, remote_nodes, false);

        let result = compare_level_nodes(&local_hashes, &response);

        assert_eq!(result.matching.len(), 2);
        assert!(result.differing.is_empty());
        assert!(result.local_missing.is_empty());
        assert!(result.remote_missing.is_empty());
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_level_nodes_all_local_missing() {
        let local_hashes: HashMap<[u8; 32], [u8; 32]> = HashMap::new();

        let remote_nodes = vec![
            LevelNode::internal([1; 32], [10; 32], None),
            LevelNode::internal([2; 32], [20; 32], None),
        ];
        let response = LevelWiseResponse::new(0, remote_nodes, false);

        let result = compare_level_nodes(&local_hashes, &response);

        assert!(result.matching.is_empty());
        assert!(result.differing.is_empty());
        assert_eq!(result.local_missing.len(), 2);
        assert!(result.remote_missing.is_empty());
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_level_nodes_empty_response() {
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]);
        local_hashes.insert([2; 32], [20; 32]);

        let response = LevelWiseResponse::empty(0);

        let result = compare_level_nodes(&local_hashes, &response);

        assert!(result.matching.is_empty());
        assert!(result.differing.is_empty());
        assert!(result.local_missing.is_empty());
        assert_eq!(result.remote_missing.len(), 2);
        assert!(!result.needs_sync());
    }

    // =========================================================================
    // Heuristic Function Tests
    // =========================================================================

    #[test]
    fn test_should_use_levelwise() {
        // Wide shallow tree - YES
        assert!(should_use_levelwise(2, 15));
        assert!(should_use_levelwise(1, 100));
        assert!(should_use_levelwise(0, 50));

        // Deep tree - NO
        assert!(!should_use_levelwise(3, 15));
        assert!(!should_use_levelwise(5, 100));

        // Narrow tree - NO
        assert!(!should_use_levelwise(2, 5));
        assert!(!should_use_levelwise(1, 10)); // Exactly 10 is not > 10
    }

    #[test]
    fn test_should_use_levelwise_boundary_conditions() {
        // Exactly at depth threshold (depth <= 2)
        assert!(should_use_levelwise(2, 15)); // depth = 2, <= 2
        assert!(!should_use_levelwise(3, 15)); // depth = 3, > 2

        // Exactly at children threshold (> 10)
        assert!(!should_use_levelwise(2, 10)); // exactly 10, not > 10
        assert!(should_use_levelwise(2, 11)); // 11, > 10

        // Edge cases
        assert!(should_use_levelwise(0, 100)); // depth 0 with many children
        assert!(!should_use_levelwise(0, 0)); // depth 0 with no children
    }

    // =========================================================================
    // Security / Exploit Tests
    // =========================================================================

    #[test]
    fn test_levelwise_request_memory_exhaustion_prevention() {
        // Request with maximum allowed parents should be valid
        let parents: Vec<[u8; 32]> = (0..MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let valid = LevelWiseRequest::for_parents(0, parents);
        assert!(valid.is_valid());

        // Request exceeding parent limit should be invalid
        let parents: Vec<[u8; 32]> = (0..=MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let invalid = LevelWiseRequest::for_parents(0, parents);
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_levelwise_response_memory_exhaustion_prevention() {
        // Response with maximum allowed nodes should be valid
        let nodes: Vec<LevelNode> = (0..MAX_NODES_PER_LEVEL)
            .map(|i| LevelNode::internal([i as u8; 32], [i as u8; 32], None))
            .collect();
        let valid = LevelWiseResponse::new(0, nodes, false);
        assert!(valid.is_valid());

        // Response exceeding node limit should be invalid
        let nodes: Vec<LevelNode> = (0..=MAX_NODES_PER_LEVEL)
            .map(|i| LevelNode::internal([i as u8; 32], [i as u8; 32], None))
            .collect();
        let invalid = LevelWiseResponse::new(0, nodes, false);
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_levelwise_special_values() {
        // All zeros
        let zeros_node = LevelNode::internal([0u8; 32], [0u8; 32], Some([0u8; 32]));
        assert!(zeros_node.is_valid());

        // All ones
        let ones_node = LevelNode::internal([0xFF; 32], [0xFF; 32], Some([0xFF; 32]));
        assert!(ones_node.is_valid());

        // Request with all-zeros
        let request = LevelWiseRequest::at_level(0);
        assert!(request.is_valid());

        // Response with no more levels
        let response = LevelWiseResponse::new(0, vec![zeros_node], false);
        assert!(response.is_valid());
        assert!(!response.has_more_levels);
    }

    #[test]
    fn test_levelwise_cross_validation_consistency() {
        // Verify that individual node validation is enforced in response validation
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let oversized_leaf_data =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_node = LevelNode::leaf([1; 32], [2; 32], None, oversized_leaf_data);

        // Invalid node by itself
        assert!(!invalid_node.is_valid());

        // Response containing invalid node should also be invalid
        let response = LevelWiseResponse::new(0, vec![invalid_node], false);
        assert!(!response.is_valid());
    }

    // =========================================================================
    // Additional Edge Case Tests
    // =========================================================================

    #[test]
    fn test_levelwise_request_empty_parent_ids() {
        // Empty parent_ids is valid (means "no filtering by parent")
        let request = LevelWiseRequest::for_parents(0, vec![]);
        assert!(request.is_valid());
        assert!(!request.is_full_level()); // Still not "full level" since parent_ids is Some
        assert_eq!(request.parent_count(), Some(0));
    }

    #[test]
    fn test_levelwise_request_zero_level() {
        let request = LevelWiseRequest::at_level(0);
        assert_eq!(request.level, 0);
        assert!(request.is_valid());
    }

    #[test]
    fn test_levelwise_request_single_parent() {
        let request = LevelWiseRequest::for_parents(1, vec![[42u8; 32]]);
        assert_eq!(request.parent_count(), Some(1));
        assert!(request.is_valid());
    }

    #[test]
    fn test_levelwise_response_single_node() {
        let node = LevelNode::internal([1; 32], [2; 32], None);
        let response = LevelWiseResponse::new(0, vec![node], true);

        assert_eq!(response.node_count(), 1);
        assert!(response.is_valid());
    }

    #[test]
    fn test_levelwise_response_all_leaves() {
        let leaves: Vec<LevelNode> = (0..5)
            .map(|i| {
                let leaf_data = make_leaf_data(i as u8, vec![i as u8]);
                LevelNode::leaf([i as u8; 32], [(i + 100) as u8; 32], None, leaf_data)
            })
            .collect();
        let response = LevelWiseResponse::new(0, leaves, false);

        assert_eq!(response.leaves().count(), 5);
        assert_eq!(response.internal_nodes().count(), 0);
        assert!(response.internal_node_ids().is_empty());
        assert!(response.is_valid());
    }

    #[test]
    fn test_levelwise_response_all_internal() {
        let nodes: Vec<LevelNode> = (0..5)
            .map(|i| LevelNode::internal([i as u8; 32], [(i + 100) as u8; 32], None))
            .collect();
        let response = LevelWiseResponse::new(0, nodes, true);

        assert_eq!(response.leaves().count(), 0);
        assert_eq!(response.internal_nodes().count(), 5);
        assert_eq!(response.internal_node_ids().len(), 5);
        assert!(response.is_valid());
    }

    #[test]
    fn test_level_node_with_various_parent_ids() {
        // No parent (root level children)
        let node_no_parent = LevelNode::internal([1; 32], [2; 32], None);
        assert!(node_no_parent.parent_id.is_none());
        assert!(node_no_parent.is_valid());

        // With parent
        let node_with_parent = LevelNode::internal([1; 32], [2; 32], Some([99; 32]));
        assert_eq!(node_with_parent.parent_id, Some([99; 32]));
        assert!(node_with_parent.is_valid());

        // All-zeros parent
        let node_zeros_parent = LevelNode::internal([1; 32], [2; 32], Some([0; 32]));
        assert!(node_zeros_parent.is_valid());

        // All-ones parent
        let node_ones_parent = LevelNode::internal([1; 32], [2; 32], Some([0xFF; 32]));
        assert!(node_ones_parent.is_valid());
    }

    #[test]
    fn test_level_node_leaf_with_empty_value() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![], metadata);
        let node = LevelNode::leaf([1; 32], [2; 32], None, leaf_data);

        assert!(node.is_leaf());
        assert!(node.is_valid());
    }

    #[test]
    fn test_level_node_leaf_at_max_value_size() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE], metadata);
        let node = LevelNode::leaf([1; 32], [2; 32], None, leaf_data);

        assert!(node.is_valid());
    }

    #[test]
    fn test_compare_level_nodes_both_empty() {
        let local_hashes: HashMap<[u8; 32], [u8; 32]> = HashMap::new();
        let response = LevelWiseResponse::empty(0);

        let result = compare_level_nodes(&local_hashes, &response);

        assert!(result.matching.is_empty());
        assert!(result.differing.is_empty());
        assert!(result.local_missing.is_empty());
        assert!(result.remote_missing.is_empty());
        assert_eq!(result.total_compared(), 0);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_level_nodes_all_differing() {
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]);
        local_hashes.insert([2; 32], [20; 32]);
        local_hashes.insert([3; 32], [30; 32]);

        // Remote has same IDs but different hashes
        let remote_nodes = vec![
            LevelNode::internal([1; 32], [11; 32], None), // Different hash
            LevelNode::internal([2; 32], [21; 32], None), // Different hash
            LevelNode::internal([3; 32], [31; 32], None), // Different hash
        ];
        let response = LevelWiseResponse::new(0, remote_nodes, false);

        let result = compare_level_nodes(&local_hashes, &response);

        assert!(result.matching.is_empty());
        assert_eq!(result.differing.len(), 3);
        assert!(result.local_missing.is_empty());
        assert!(result.remote_missing.is_empty());
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_level_nodes_all_remote_missing() {
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]);
        local_hashes.insert([2; 32], [20; 32]);
        local_hashes.insert([3; 32], [30; 32]);

        // Empty response - all local nodes are "remote missing"
        let response = LevelWiseResponse::empty(0);

        let result = compare_level_nodes(&local_hashes, &response);

        assert!(result.matching.is_empty());
        assert!(result.differing.is_empty());
        assert!(result.local_missing.is_empty());
        assert_eq!(result.remote_missing.len(), 3);
        assert!(!result.needs_sync()); // Remote missing doesn't require local sync
    }

    #[test]
    fn test_level_compare_result_only_differing() {
        let mut result = LevelCompareResult::default();
        result.differing.push([1; 32]);
        result.differing.push([2; 32]);

        assert!(result.needs_sync());
        assert_eq!(result.nodes_to_process().len(), 2);
        assert_eq!(result.total_compared(), 2);
    }

    #[test]
    fn test_level_compare_result_only_local_missing() {
        let mut result = LevelCompareResult::default();
        result.local_missing.push([1; 32]);
        result.local_missing.push([2; 32]);

        assert!(result.needs_sync());
        assert_eq!(result.nodes_to_process().len(), 2);
        assert_eq!(result.total_compared(), 2);
    }

    #[test]
    fn test_level_compare_result_only_matching() {
        let mut result = LevelCompareResult::default();
        result.matching.push([1; 32]);
        result.matching.push([2; 32]);
        result.matching.push([3; 32]);

        assert!(!result.needs_sync());
        assert!(result.nodes_to_process().is_empty());
        assert_eq!(result.total_compared(), 3);
    }

    // =========================================================================
    // Security / Exploit Tests - Extended
    // =========================================================================

    #[test]
    fn test_levelwise_request_level_overflow_prevention() {
        // Simulate a malicious request with usize::MAX level
        // This tests that validation catches extreme values
        let mut request = LevelWiseRequest::at_level(0);
        request.level = usize::MAX;

        assert!(!request.is_valid());
    }

    #[test]
    fn test_levelwise_response_level_overflow_prevention() {
        // Response with usize::MAX level should be invalid
        let mut response = LevelWiseResponse::empty(0);
        response.level = usize::MAX;

        assert!(!response.is_valid());
    }

    #[test]
    fn test_levelwise_request_both_limits_at_boundary() {
        // Request at both level and parent limits
        let parents: Vec<[u8; 32]> = (0..MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let request = LevelWiseRequest {
            level: MAX_LEVELWISE_DEPTH,
            parent_ids: Some(parents),
        };

        assert!(request.is_valid());

        // Just over one limit should fail
        let parents_over: Vec<[u8; 32]> = (0..=MAX_PARENTS_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let request_over = LevelWiseRequest {
            level: MAX_LEVELWISE_DEPTH,
            parent_ids: Some(parents_over),
        };
        assert!(!request_over.is_valid());
    }

    #[test]
    fn test_levelwise_response_with_mixed_valid_invalid_nodes() {
        // Response with mostly valid nodes but one invalid
        let valid_nodes: Vec<LevelNode> = (0..5)
            .map(|i| LevelNode::internal([i as u8; 32], [i as u8; 32], None))
            .collect();

        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let oversized_leaf_data =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_node = LevelNode::leaf([99; 32], [99; 32], None, oversized_leaf_data);

        let mut all_nodes = valid_nodes;
        all_nodes.push(invalid_node);

        let response = LevelWiseResponse::new(0, all_nodes, false);

        // One invalid node makes the whole response invalid
        assert!(!response.is_valid());
    }

    #[test]
    fn test_levelwise_serialization_roundtrip_with_edge_values() {
        // Request with boundary values
        let parents: Vec<[u8; 32]> = vec![[0xFF; 32], [0u8; 32]];
        let request = LevelWiseRequest {
            level: MAX_LEVELWISE_DEPTH,
            parent_ids: Some(parents),
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: LevelWiseRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert!(decoded.is_valid());
    }

    #[test]
    fn test_levelwise_response_serialization_roundtrip_with_max_level() {
        let node = LevelNode::internal([0xFF; 32], [0u8; 32], Some([0x55; 32]));
        let response = LevelWiseResponse::new(MAX_LEVELWISE_DEPTH, vec![node], false);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: LevelWiseResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert!(decoded.is_valid());
        assert_eq!(decoded.level, MAX_LEVELWISE_DEPTH);
    }

    #[test]
    fn test_levelwise_malicious_deserialization_level() {
        // Manually construct bytes simulating a malicious response with extreme level
        // Response structure: level (usize) + nodes vec + has_more_levels (bool)

        // Build a minimal valid-looking response but with extreme level
        let node = LevelNode::internal([1; 32], [2; 32], None);
        let mut response = LevelWiseResponse::new(0, vec![node], false);
        response.level = MAX_LEVELWISE_DEPTH + 100; // Simulate tampered deserialization

        // Validation should catch this
        assert!(!response.is_valid());
    }

    #[test]
    fn test_compare_level_nodes_with_duplicate_ids_in_response() {
        // Malicious response with duplicate node IDs
        let mut local_hashes = HashMap::new();
        local_hashes.insert([1; 32], [10; 32]);

        // Remote has duplicate IDs (malicious)
        let remote_nodes = vec![
            LevelNode::internal([1; 32], [10; 32], None), // First occurrence - matches
            LevelNode::internal([1; 32], [11; 32], None), // Duplicate ID, different hash
        ];
        let response = LevelWiseResponse::new(0, remote_nodes, false);

        let result = compare_level_nodes(&local_hashes, &response);

        // First occurrence matches, second differs
        // This is technically valid behavior - compare processes each node
        assert_eq!(result.matching.len(), 1);
        assert_eq!(result.differing.len(), 1);
        // Same ID appears in both categories - application should handle this
    }

    #[test]
    fn test_levelwise_node_with_zero_hash() {
        // Zero hash is technically valid (though unusual)
        let node = LevelNode::internal([1; 32], [0u8; 32], None);
        assert!(node.is_valid());

        // Zero ID is also valid
        let node_zero_id = LevelNode::internal([0u8; 32], [1; 32], None);
        assert!(node_zero_id.is_valid());
    }

    #[test]
    fn test_levelwise_leaf_with_all_metadata_variants() {
        // Test with various CRDT types
        let crdt_types = [
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
        ];

        for crdt_type in crdt_types {
            let metadata = LeafMetadata::new(crdt_type.clone(), 12345, [1; 32])
                .with_version(100)
                .with_parent([2; 32]);
            let leaf_data = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);
            let node = LevelNode::leaf([1; 32], [2; 32], None, leaf_data);

            assert!(node.is_valid());

            // Verify roundtrip serialization
            let encoded = borsh::to_vec(&node).expect("serialize");
            let decoded: LevelNode = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(node, decoded);
        }
    }

    #[test]
    fn test_should_use_levelwise_extreme_values() {
        // Very large depth
        assert!(!should_use_levelwise(usize::MAX, 100));

        // Very large children count
        assert!(should_use_levelwise(2, usize::MAX));

        // Both extreme
        assert!(!should_use_levelwise(usize::MAX, usize::MAX));

        // Zero both
        assert!(!should_use_levelwise(0, 0));
    }

    #[test]
    fn test_levelwise_response_validation_with_deeply_nested_invalid_data() {
        // Create a response where validity depends on nested leaf validation
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);

        // Valid leaf with exactly MAX size
        let valid_leaf_data =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE], metadata.clone());
        let valid_node = LevelNode::leaf([1; 32], [2; 32], None, valid_leaf_data);
        assert!(valid_node.is_valid());

        // Invalid leaf with MAX+1 size
        let invalid_leaf_data =
            TreeLeafData::new([2; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_node = LevelNode::leaf([2; 32], [3; 32], None, invalid_leaf_data);
        assert!(!invalid_node.is_valid());

        // Response with only the valid node
        let valid_response = LevelWiseResponse::new(0, vec![valid_node.clone()], false);
        assert!(valid_response.is_valid());

        // Response with the invalid node
        let invalid_response = LevelWiseResponse::new(0, vec![invalid_node], false);
        assert!(!invalid_response.is_valid());
    }

    #[test]
    fn test_level_compare_result_nodes_to_process_order() {
        // Verify nodes_to_process returns differing first, then local_missing
        let mut result = LevelCompareResult::default();
        result.differing.push([1; 32]);
        result.differing.push([2; 32]);
        result.local_missing.push([3; 32]);
        result.local_missing.push([4; 32]);

        let to_process = result.nodes_to_process();
        assert_eq!(to_process.len(), 4);
        // Differing should come first
        assert_eq!(to_process[0], [1; 32]);
        assert_eq!(to_process[1], [2; 32]);
        // Then local_missing
        assert_eq!(to_process[2], [3; 32]);
        assert_eq!(to_process[3], [4; 32]);
    }

    #[test]
    fn test_levelwise_response_multiple_invalid_nodes() {
        // Response with multiple invalid nodes at different positions
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let oversized_data = vec![0u8; MAX_LEAF_VALUE_SIZE + 1];

        let nodes = vec![
            LevelNode::internal([1; 32], [1; 32], None), // Valid
            LevelNode::leaf(
                [2; 32],
                [2; 32],
                None,
                TreeLeafData::new([2; 32], oversized_data.clone(), metadata.clone()),
            ), // Invalid
            LevelNode::internal([3; 32], [3; 32], None), // Valid
            LevelNode::leaf(
                [4; 32],
                [4; 32],
                None,
                TreeLeafData::new([4; 32], oversized_data, metadata),
            ), // Invalid
        ];

        let response = LevelWiseResponse::new(0, nodes, false);
        assert!(!response.is_valid());
    }

    #[test]
    fn test_levelwise_empty_structures_are_valid() {
        // Empty request (no parent filter)
        let empty_request = LevelWiseRequest::at_level(0);
        assert!(empty_request.is_valid());

        // Empty response
        let empty_response = LevelWiseResponse::empty(0);
        assert!(empty_response.is_valid());
        assert!(empty_response.is_empty());

        // Empty compare result
        let empty_result = LevelCompareResult::default();
        assert!(!empty_result.needs_sync());
        assert!(empty_result.nodes_to_process().is_empty());
        assert_eq!(empty_result.total_compared(), 0);
    }
}
