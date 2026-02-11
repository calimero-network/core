//! HashComparison sync types (CIP §4 - State Machine, STATE-BASED branch).
//!
//! Types for Merkle tree traversal and hash-based synchronization.

use std::collections::HashSet;

use borsh::{BorshDeserialize, BorshSerialize};

// Re-export the unified CrdtType from primitives (consolidated per issue #1912)
pub use calimero_primitives::crdt::CrdtType;

// =============================================================================
// Constants
// =============================================================================

/// Maximum nodes per response to prevent memory exhaustion.
///
/// Limits the size of `TreeNodeResponse::nodes` to prevent DoS attacks
/// from malicious peers sending oversized responses.
pub const MAX_NODES_PER_RESPONSE: usize = 1000;

/// Maximum children per node (typical Merkle trees use binary or small fanout).
///
/// This limit prevents memory exhaustion from malicious nodes with excessive children.
pub const MAX_CHILDREN_PER_NODE: usize = 256;

/// Maximum size for leaf value data (1 MB).
///
/// Prevents memory exhaustion from malicious peers sending oversized leaf values.
/// This should be sufficient for most entity data while protecting against DoS.
pub const MAX_LEAF_VALUE_SIZE: usize = 1_048_576;

/// Maximum allowed tree depth for traversal requests.
///
/// This limit prevents resource exhaustion from malicious peers requesting
/// extremely deep traversals. Most practical Merkle trees have depth < 32.
pub const MAX_TREE_DEPTH: usize = 64;

// =============================================================================
// Tree Node Request/Response
// =============================================================================

/// Request to traverse the Merkle tree for hash comparison.
///
/// Used for recursive tree traversal to identify differing entities.
/// Start at root, request children, compare hashes, recurse on differences.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeRequest {
    /// ID of the node to request (root hash or internal node hash).
    pub node_id: [u8; 32],

    /// Maximum depth to traverse from this node (private to enforce validation).
    ///
    /// Use `depth()` accessor which always clamps to MAX_TREE_DEPTH.
    /// Use `with_depth()` constructor to set a depth limit.
    max_depth: Option<usize>,
}

impl TreeNodeRequest {
    /// Create a request for a specific node.
    #[must_use]
    pub fn new(node_id: [u8; 32]) -> Self {
        Self {
            node_id,
            max_depth: None,
        }
    }

    /// Create a request with depth limit.
    #[must_use]
    pub fn with_depth(node_id: [u8; 32], max_depth: usize) -> Self {
        Self {
            node_id,
            // Clamp to MAX_TREE_DEPTH to prevent resource exhaustion
            max_depth: Some(max_depth.min(MAX_TREE_DEPTH)),
        }
    }

    /// Create a request for the root node.
    #[must_use]
    pub fn root(root_hash: [u8; 32]) -> Self {
        Self::new(root_hash)
    }

    /// Get the validated depth limit.
    ///
    /// Always clamps to MAX_TREE_DEPTH, even if raw field was set to a larger
    /// value (e.g., via deserialization from an untrusted source).
    ///
    /// Use this instead of accessing `max_depth` directly when processing requests.
    #[must_use]
    pub fn depth(&self) -> Option<usize> {
        self.max_depth.map(|d| d.min(MAX_TREE_DEPTH))
    }
}

/// Response containing tree nodes for hash comparison.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeResponse {
    /// Nodes in the requested subtree.
    ///
    /// Limited to MAX_NODES_PER_RESPONSE entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub nodes: Vec<TreeNode>,

    /// True if the requested node was not found.
    pub not_found: bool,
}

impl TreeNodeResponse {
    /// Create a response with nodes.
    #[must_use]
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        Self {
            nodes,
            not_found: false,
        }
    }

    /// Create a not-found response.
    #[must_use]
    pub fn not_found() -> Self {
        Self {
            nodes: vec![],
            not_found: true,
        }
    }

    /// Check if response contains any leaf nodes.
    #[must_use]
    pub fn has_leaves(&self) -> bool {
        self.nodes.iter().any(|n| n.is_leaf())
    }

    /// Get an iterator over leaf nodes in response.
    ///
    /// Returns an iterator rather than allocating a Vec, which is more
    /// efficient for single-pass iteration.
    pub fn leaves(&self) -> impl Iterator<Item = &TreeNode> {
        self.nodes.iter().filter(|n| n.is_leaf())
    }

    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// memory exhaustion attacks. Validates both response size and all
    /// contained nodes.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.nodes.len() <= MAX_NODES_PER_RESPONSE && self.nodes.iter().all(TreeNode::is_valid)
    }
}

// =============================================================================
// Tree Node
// =============================================================================

/// A node in the Merkle tree.
///
/// Can be either an internal node (has children) or a leaf node (has data).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNode {
    /// Node ID - stable identifier derived from the node's position/key in the tree.
    ///
    /// For internal nodes: typically hash of path or concatenation of child keys.
    /// For leaf nodes: typically hash of the entity key.
    /// This ID remains stable even when content changes.
    pub id: [u8; 32],

    /// Merkle hash - changes when subtree content changes.
    ///
    /// For internal nodes: hash of all children's hashes (propagates changes up).
    /// For leaf nodes: hash of the leaf data (key + value + metadata).
    /// Used for efficient comparison: if hashes match, subtrees are identical.
    pub hash: [u8; 32],

    /// Child node IDs (empty for leaf nodes).
    ///
    /// Typically limited to MAX_CHILDREN_PER_NODE. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub children: Vec<[u8; 32]>,

    /// Leaf data (present only for leaf nodes).
    pub leaf_data: Option<TreeLeafData>,
}

impl TreeNode {
    /// Create an internal node.
    #[must_use]
    pub fn internal(id: [u8; 32], hash: [u8; 32], children: Vec<[u8; 32]>) -> Self {
        Self {
            id,
            hash,
            children,
            leaf_data: None,
        }
    }

    /// Create a leaf node.
    #[must_use]
    pub fn leaf(id: [u8; 32], hash: [u8; 32], data: TreeLeafData) -> Self {
        Self {
            id,
            hash,
            children: vec![],
            leaf_data: Some(data),
        }
    }

    /// Check if node is within valid bounds and structurally valid.
    ///
    /// Call this after deserializing from untrusted sources.
    /// Validates:
    /// - Children count within MAX_CHILDREN_PER_NODE
    /// - Structural invariant: must have exactly one of children OR leaf_data
    /// - Leaf data validity (value size within limits)
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check children count
        if self.children.len() > MAX_CHILDREN_PER_NODE {
            return false;
        }

        // Check structural invariant: must have exactly one of children or data
        // - Internal nodes: non-empty children, no leaf_data
        // - Leaf nodes: empty children, has leaf_data
        let has_children = !self.children.is_empty();
        let has_data = self.leaf_data.is_some();
        if has_children == has_data {
            // Invalid: either both present (ambiguous) or both absent (empty)
            return false;
        }

        // Validate leaf data if present
        if let Some(ref leaf_data) = self.leaf_data {
            if !leaf_data.is_valid() {
                return false;
            }
        }

        true
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

    /// Get number of children (0 for leaf nodes).
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

// =============================================================================
// Tree Leaf Data
// =============================================================================

/// Data stored at a leaf node (entity).
///
/// Contains ALL information needed for CRDT merge on the receiving side.
/// CRITICAL: `metadata` MUST include `crdt_type` for proper merge.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeLeafData {
    /// Entity key (unique identifier within collection).
    pub key: [u8; 32],

    /// Serialized entity value.
    pub value: Vec<u8>,

    /// Entity metadata including crdt_type.
    /// CRITICAL: Must be included for CRDT merge to work correctly.
    pub metadata: LeafMetadata,
}

impl TreeLeafData {
    /// Create leaf data.
    #[must_use]
    pub fn new(key: [u8; 32], value: Vec<u8>, metadata: LeafMetadata) -> Self {
        Self {
            key,
            value,
            metadata,
        }
    }

    /// Check if leaf data is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.value.len() <= MAX_LEAF_VALUE_SIZE
    }
}

/// Metadata for a leaf entity.
///
/// Minimal metadata needed for CRDT merge during sync.
///
/// This is a wire-protocol-optimized subset of `calimero_storage::Metadata`.
/// It contains only the fields needed for sync operations, avoiding larger
/// fields like `field_name: String` that aren't needed over the wire.
///
/// When receiving entities, implementations should map this to/from the
/// storage layer's `Metadata` type.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LeafMetadata {
    /// CRDT type for proper merge semantics.
    pub crdt_type: CrdtType,

    /// HLC timestamp of last modification.
    pub hlc_timestamp: u64,

    /// Version counter (for some CRDT types).
    pub version: u64,

    /// Collection ID this entity belongs to.
    pub collection_id: [u8; 32],

    /// Optional parent entity ID (for nested structures).
    pub parent_id: Option<[u8; 32]>,
}

impl LeafMetadata {
    /// Create metadata with required fields.
    #[must_use]
    pub fn new(crdt_type: CrdtType, hlc_timestamp: u64, collection_id: [u8; 32]) -> Self {
        Self {
            crdt_type,
            hlc_timestamp,
            version: 0,
            collection_id,
            parent_id: None,
        }
    }

    /// Set version.
    #[must_use]
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Set parent ID.
    #[must_use]
    pub fn with_parent(mut self, parent_id: [u8; 32]) -> Self {
        self.parent_id = Some(parent_id);
        self
    }
}

// =============================================================================
// Tree Compare Result
// =============================================================================

/// Result of comparing two tree nodes.
///
/// Used for Merkle tree traversal during HashComparison sync.
/// Identifies which children need further traversal in both directions.
///
/// Note: Borsh derives are included for consistency with other sync types and
/// potential future use in batched comparison responses over the wire.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum TreeCompareResult {
    /// Hashes match - no sync needed for this subtree.
    Equal,
    /// Hashes differ - need to recurse or fetch leaf.
    ///
    /// For internal nodes: lists children to recurse into.
    /// For leaf nodes: all vecs will be empty, but Different still indicates
    /// that the leaf data needs to be fetched and merged bidirectionally.
    Different {
        /// IDs of children in remote but not in local (need to fetch).
        remote_only_children: Vec<[u8; 32]>,
        /// IDs of children in local but not in remote (for bidirectional sync).
        local_only_children: Vec<[u8; 32]>,
        /// IDs of children present on both sides that need recursive comparison.
        /// These are the primary candidates for recursion when parent hashes differ.
        common_children: Vec<[u8; 32]>,
    },
    /// Local node missing - need to fetch from remote.
    LocalMissing,
    /// Remote node missing - local has data that remote doesn't.
    /// For bidirectional sync, this means we may need to push to remote.
    RemoteMissing,
}

impl TreeCompareResult {
    /// Check if sync (pull from remote) is needed.
    ///
    /// Returns true if local needs data from remote.
    #[must_use]
    pub fn needs_sync(&self) -> bool {
        !matches!(self, Self::Equal | Self::RemoteMissing)
    }

    /// Check if push (send to remote) is needed for bidirectional sync.
    ///
    /// Returns true if local has data that remote doesn't:
    /// - `RemoteMissing`: entire local subtree needs pushing
    /// - `Different` with `local_only_children`: those children need pushing
    /// - `Different` with all empty vecs: this is a **leaf node comparison** where
    ///   hashes differ, meaning local leaf data needs pushing for CRDT merge
    #[must_use]
    pub fn needs_push(&self) -> bool {
        match self {
            Self::RemoteMissing => true,
            Self::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                // Push needed if we have local-only children
                if !local_only_children.is_empty() {
                    return true;
                }
                // Leaf node detection: when all child vecs are empty but hashes differed,
                // we compared two leaf nodes with different content. The local leaf data
                // needs to be pushed for bidirectional CRDT merge.
                remote_only_children.is_empty() && common_children.is_empty()
            }
            _ => false,
        }
    }
}

// =============================================================================
// Compare Function
// =============================================================================

/// Compare local and remote tree nodes.
///
/// Returns which children (if any) need further traversal in both directions.
/// This supports bidirectional sync where both nodes may have unique data.
///
/// # Arguments
/// * `local` - Local tree node, or None if not present locally
/// * `remote` - Remote tree node, or None if not present on remote
///
/// # Precondition
/// When both nodes are present, they must represent the same tree position
/// (i.e., have matching IDs). Comparing nodes at different positions is a
/// caller bug and will trigger a debug assertion.
///
/// # Returns
/// * `Equal` - Hashes match, no sync needed
/// * `Different` - Hashes differ, contains children needing traversal
/// * `LocalMissing` - Need to fetch from remote
/// * `RemoteMissing` - Local has data remote doesn't (for bidirectional push)
#[must_use]
pub fn compare_tree_nodes(
    local: Option<&TreeNode>,
    remote: Option<&TreeNode>,
) -> TreeCompareResult {
    match (local, remote) {
        (None, None) => TreeCompareResult::Equal,
        (None, Some(_)) => TreeCompareResult::LocalMissing,
        (Some(_), None) => TreeCompareResult::RemoteMissing,
        (Some(local_node), Some(remote_node)) => {
            // Verify precondition: nodes must represent the same tree position
            debug_assert_eq!(
                local_node.id, remote_node.id,
                "compare_tree_nodes called with nodes at different tree positions"
            );

            if local_node.hash == remote_node.hash {
                TreeCompareResult::Equal
            } else {
                // Use HashSet for O(1) lookups instead of O(n) Vec::contains
                let local_children: HashSet<&[u8; 32]> = local_node.children.iter().collect();
                let remote_children: HashSet<&[u8; 32]> = remote_node.children.iter().collect();

                // Children in remote but not in local (need to fetch)
                let remote_only_children: Vec<[u8; 32]> = remote_node
                    .children
                    .iter()
                    .filter(|child_id| !local_children.contains(child_id))
                    .copied()
                    .collect();

                // Children in local but not in remote (for bidirectional sync)
                let local_only_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| !remote_children.contains(child_id))
                    .copied()
                    .collect();

                // Children present on both sides - these are the primary candidates
                // for recursive comparison when parent hashes differ
                let common_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| remote_children.contains(child_id))
                    .copied()
                    .collect();

                TreeCompareResult::Different {
                    remote_only_children,
                    local_only_children,
                    common_children,
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_node_request_roundtrip() {
        let request = TreeNodeRequest::with_depth([1; 32], 3);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: TreeNodeRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.depth(), Some(3));
    }

    #[test]
    fn test_tree_node_request_root() {
        let root_hash = [42; 32];
        let request = TreeNodeRequest::root(root_hash);

        assert_eq!(request.node_id, root_hash);
        assert!(request.depth().is_none());
    }

    #[test]
    fn test_tree_node_internal() {
        let node = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);

        assert!(node.is_internal());
        assert!(!node.is_leaf());
        assert_eq!(node.child_count(), 2);
        assert!(node.leaf_data.is_none());
    }

    #[test]
    fn test_tree_node_leaf() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 12345, [5; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        assert!(node.is_leaf());
        assert!(!node.is_internal());
        assert_eq!(node.child_count(), 0);
        assert!(node.leaf_data.is_some());
    }

    #[test]
    fn test_tree_node_roundtrip() {
        let metadata = LeafMetadata::new(CrdtType::UnorderedMap, 999, [6; 32])
            .with_version(5)
            .with_parent([7; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![4, 5, 6], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        let encoded = borsh::to_vec(&node).expect("serialize");
        let decoded: TreeNode = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(node, decoded);
    }

    #[test]
    fn test_tree_node_response_roundtrip() {
        let internal = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);
        let metadata = LeafMetadata::new(CrdtType::Rga, 100, [4; 32]);
        let leaf_data = TreeLeafData::new([5; 32], vec![7, 8, 9], metadata);
        let leaf = TreeNode::leaf([6; 32], [7; 32], leaf_data);

        let response = TreeNodeResponse::new(vec![internal, leaf]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: TreeNodeResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert!(decoded.has_leaves());
        assert_eq!(decoded.leaves().count(), 1);
    }

    #[test]
    fn test_tree_node_response_not_found() {
        let response = TreeNodeResponse::not_found();

        assert!(response.not_found);
        assert!(response.nodes.is_empty());
        assert!(!response.has_leaves());
    }

    #[test]
    fn test_leaf_metadata_builder() {
        let metadata = LeafMetadata::new(CrdtType::PnCounter, 500, [1; 32])
            .with_version(10)
            .with_parent([2; 32]);

        assert_eq!(metadata.crdt_type, CrdtType::PnCounter);
        assert_eq!(metadata.hlc_timestamp, 500);
        assert_eq!(metadata.version, 10);
        assert_eq!(metadata.parent_id, Some([2; 32]));
    }

    #[test]
    fn test_crdt_type_variants() {
        let types = vec![
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Custom("test".to_string()),
        ];

        for crdt_type in types {
            let encoded = borsh::to_vec(&crdt_type).expect("serialize");
            let decoded: CrdtType = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(crdt_type, decoded);
        }
    }

    #[test]
    fn test_compare_tree_nodes_equal() {
        let local = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));
        assert_eq!(result, TreeCompareResult::Equal);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_missing() {
        let remote = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(None, Some(&remote));
        assert_eq!(result, TreeCompareResult::LocalMissing);
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_different() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [3; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children: _,
                common_children,
            } => {
                // [3; 32] is in remote but not in local
                assert!(remote_only_children.contains(&[3; 32]));
                // [2; 32] is common to both sides
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
        assert!(result.needs_sync());
    }

    #[test]
    fn test_tree_compare_result_needs_sync() {
        assert!(!TreeCompareResult::Equal.needs_sync());
        assert!(!TreeCompareResult::RemoteMissing.needs_sync());
        assert!(TreeCompareResult::LocalMissing.needs_sync());
        assert!(TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        }
        .needs_sync());
    }

    #[test]
    fn test_tree_compare_result_roundtrip() {
        let variants = vec![
            TreeCompareResult::Equal,
            TreeCompareResult::LocalMissing,
            TreeCompareResult::RemoteMissing,
            TreeCompareResult::Different {
                remote_only_children: vec![[1; 32], [2; 32]],
                local_only_children: vec![[3; 32]],
                common_children: vec![[4; 32], [5; 32], [6; 32]],
            },
            TreeCompareResult::Different {
                remote_only_children: vec![],
                local_only_children: vec![],
                common_children: vec![],
            },
        ];

        for original in variants {
            let encoded = borsh::to_vec(&original).expect("encode");
            let decoded: TreeCompareResult = borsh::from_slice(&encoded).expect("decode");
            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn test_compare_tree_nodes_leaf_content_differs() {
        let local_metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let local_leaf = TreeLeafData::new([10; 32], vec![1, 2, 3], local_metadata);
        let local = TreeNode::leaf([1; 32], [100; 32], local_leaf);

        let remote_metadata = LeafMetadata::new(CrdtType::LwwRegister, 200, [1; 32]);
        let remote_leaf = TreeLeafData::new([10; 32], vec![4, 5, 6], remote_metadata);
        let remote = TreeNode::leaf([1; 32], [200; 32], remote_leaf);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                assert!(remote_only_children.is_empty());
                assert!(local_only_children.is_empty());
                assert!(common_children.is_empty());
            }
            _ => panic!("Expected Different result for leaves with different content"),
        }
        assert!(result.needs_sync());
        assert!(result.needs_push());
    }

    #[test]
    fn test_compare_tree_nodes_remote_missing() {
        let local = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(Some(&local), None);
        assert_eq!(result, TreeCompareResult::RemoteMissing);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_only_children() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32], [3; 32], [4; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [5; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                assert!(remote_only_children.contains(&[5; 32]));
                assert!(local_only_children.contains(&[3; 32]));
                assert!(local_only_children.contains(&[4; 32]));
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
    }

    #[test]
    fn test_tree_node_request_max_depth_validation() {
        let request = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH);
        assert_eq!(request.max_depth, Some(MAX_TREE_DEPTH));

        let excessive = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH + 100);
        assert_eq!(excessive.max_depth, Some(MAX_TREE_DEPTH));
    }

    #[test]
    fn test_tree_node_request_depth_accessor() {
        // Test that depth() clamps even when raw max_depth was set to excessive value
        // (simulating deserialization from an untrusted source)
        let request = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH);
        assert_eq!(request.depth(), Some(MAX_TREE_DEPTH));

        // Simulate deserializing a malicious request with excessive depth
        let malicious_request = TreeNodeRequest::with_depth([1; 32], usize::MAX);
        // with_depth clamps on construction, verify depth() still returns clamped value
        assert_eq!(malicious_request.depth(), Some(MAX_TREE_DEPTH));

        let request_none = TreeNodeRequest::new([1; 32]);
        assert_eq!(request_none.depth(), None);
    }

    #[test]
    fn test_tree_node_response_validation() {
        let valid_response =
            TreeNodeResponse::new(vec![TreeNode::internal([1; 32], [2; 32], vec![[3; 32]])]);
        assert!(valid_response.is_valid());

        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let leaf_response =
            TreeNodeResponse::new(vec![TreeNode::leaf([1; 32], [2; 32], leaf_data)]);
        assert!(leaf_response.is_valid());

        let mut nodes = Vec::new();
        for i in 0..MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let at_limit = TreeNodeResponse::new(nodes);
        assert!(at_limit.is_valid());
    }

    #[test]
    fn test_tree_node_validation() {
        let valid = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);
        assert!(valid.is_valid());

        let children: Vec<[u8; 32]> = (0..MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let at_limit = TreeNode::internal([1; 32], [2; 32], children);
        assert!(at_limit.is_valid());

        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let over_limit = TreeNode::internal([1; 32], [2; 32], over_children);
        assert!(!over_limit.is_valid());

        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let invalid_node = TreeNode {
            id: [1; 32],
            hash: [2; 32],
            children: vec![[3; 32]],
            leaf_data: Some(leaf_data),
        };
        assert!(!invalid_node.is_valid());

        let valid_metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let valid_leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], valid_metadata);
        let valid_leaf = TreeNode::leaf([1; 32], [2; 32], valid_leaf_data);
        assert!(valid_leaf.is_valid());

        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        assert!(!empty_node.is_valid());
    }

    #[test]
    fn test_tree_node_response_validation_over_limit() {
        let mut nodes = Vec::new();
        for i in 0..=MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let over_limit = TreeNodeResponse::new(nodes);
        assert!(!over_limit.is_valid());

        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let invalid_node = TreeNode::internal([1; 32], [2; 32], over_children);
        let response_with_invalid = TreeNodeResponse::new(vec![invalid_node]);
        assert!(!response_with_invalid.is_valid());

        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        let response_with_empty = TreeNodeResponse::new(vec![empty_node]);
        assert!(!response_with_empty.is_valid());
    }

    #[test]
    fn test_tree_leaf_data_validation() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);

        let valid = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata.clone());
        assert!(valid.is_valid());

        let at_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE];
        let at_limit = TreeLeafData::new([1; 32], at_limit_value, metadata.clone());
        assert!(at_limit.is_valid());

        let over_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE + 1];
        let over_limit = TreeLeafData::new([1; 32], over_limit_value, metadata);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_tree_compare_result_needs_push() {
        assert!(TreeCompareResult::RemoteMissing.needs_push());

        let with_local_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![[1; 32]],
            common_children: vec![],
        };
        assert!(with_local_only.needs_push());

        let with_remote_only = TreeCompareResult::Different {
            remote_only_children: vec![[1; 32]],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(!with_remote_only.needs_push());

        let with_common_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![[1; 32]],
        };
        assert!(!with_common_only.needs_push());

        let differing_leaves = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(differing_leaves.needs_push());

        assert!(!TreeCompareResult::Equal.needs_push());
        assert!(!TreeCompareResult::LocalMissing.needs_push());
    }
}
