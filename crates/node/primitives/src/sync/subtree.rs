//! SubtreePrefetch sync types (CIP Appendix B - Protocol Selection Matrix).
//!
//! Types for subtree prefetch-based synchronization, optimized for deep trees
//! with clustered changes.
//!
//! # When to Use
//!
//! - `max_depth > 3` (deep trees)
//! - `divergence < 20%`
//! - Changes are clustered in subtrees
//!
//! # Trade-offs
//!
//! | Aspect        | HashComparison     | SubtreePrefetch      |
//! |---------------|--------------------|-----------------------|
//! | Round trips   | O(depth)           | O(1) per subtree      |
//! | Data transfer | Minimal            | May over-fetch        |
//! | Best for      | Scattered changes  | Clustered changes     |
//!
//! # Validation
//!
//! All types have `is_valid()` methods that should be called after deserializing
//! from untrusted sources to prevent resource exhaustion attacks.

use borsh::{BorshDeserialize, BorshSerialize};

use super::hash_comparison::TreeLeafData;

// =============================================================================
// Constants
// =============================================================================

/// Default maximum depth for subtree prefetch.
///
/// Limits how deep into a subtree we fetch to avoid over-fetching.
pub const DEFAULT_SUBTREE_MAX_DEPTH: usize = 5;

/// Maximum allowed depth for subtree traversal requests.
///
/// This limit prevents resource exhaustion from malicious peers requesting
/// extremely deep traversals. Aligned with `hash_comparison::MAX_TREE_DEPTH`.
pub const MAX_SUBTREE_DEPTH: usize = 64;

/// Maximum number of subtree roots in a single request.
///
/// Limits the size of `SubtreePrefetchRequest::subtree_roots` to prevent
/// DoS attacks from malicious peers sending oversized requests.
pub const MAX_SUBTREES_PER_REQUEST: usize = 100;

/// Maximum number of entities per subtree.
///
/// Limits the size of `SubtreeData::entities` to prevent memory exhaustion
/// from malicious peers sending oversized subtree responses.
pub const MAX_ENTITIES_PER_SUBTREE: usize = 10_000;

/// Maximum total entities across all subtrees in a response.
///
/// Even if each subtree is within its individual limit, the total could still
/// cause memory exhaustion. This bounds the entire response.
/// `MAX_SUBTREES_PER_REQUEST * MAX_ENTITIES_PER_SUBTREE` would be 1M entities,
/// so we set a more reasonable limit.
pub const MAX_TOTAL_ENTITIES: usize = 100_000;

// =============================================================================
// SubtreePrefetch Request/Response
// =============================================================================

/// Request for subtree prefetch-based sync.
///
/// Fetches entire subtrees when divergence is detected in deep trees.
/// More efficient than HashComparison when changes are clustered.
///
/// Use when:
/// - max_depth > 3
/// - divergence < 20%
/// - Changes are clustered in subtrees
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SubtreePrefetchRequest {
    /// Root hashes of subtrees to fetch.
    ///
    /// Limited to MAX_SUBTREES_PER_REQUEST entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub subtree_roots: Vec<[u8; 32]>,

    /// Maximum depth to traverse within each subtree (None = unlimited).
    ///
    /// Use `depth()` accessor which always clamps to MAX_SUBTREE_DEPTH.
    /// Use `with_depth()` constructor to set a depth limit.
    pub max_depth: Option<usize>,
}

impl SubtreePrefetchRequest {
    /// Create a new subtree prefetch request.
    #[must_use]
    pub fn new(subtree_roots: Vec<[u8; 32]>) -> Self {
        Self {
            subtree_roots,
            max_depth: Some(DEFAULT_SUBTREE_MAX_DEPTH),
        }
    }

    /// Create a request with custom depth limit.
    ///
    /// Depth is clamped to MAX_SUBTREE_DEPTH to prevent resource exhaustion.
    #[must_use]
    pub fn with_depth(subtree_roots: Vec<[u8; 32]>, max_depth: usize) -> Self {
        Self {
            subtree_roots,
            // Clamp to MAX_SUBTREE_DEPTH to prevent resource exhaustion
            max_depth: Some(max_depth.min(MAX_SUBTREE_DEPTH)),
        }
    }

    /// Create a request with unlimited depth (use carefully).
    ///
    /// Note: Even with unlimited depth, the `depth()` accessor will clamp
    /// to MAX_SUBTREE_DEPTH when processing requests.
    #[must_use]
    pub fn unlimited_depth(subtree_roots: Vec<[u8; 32]>) -> Self {
        Self {
            subtree_roots,
            max_depth: None,
        }
    }

    /// Get the validated depth limit.
    ///
    /// Always clamps to MAX_SUBTREE_DEPTH, even if raw field was set to a larger
    /// value (e.g., via deserialization from an untrusted source).
    ///
    /// Use this instead of accessing `max_depth` directly when processing requests.
    #[must_use]
    pub fn depth(&self) -> Option<usize> {
        self.max_depth.map(|d| d.min(MAX_SUBTREE_DEPTH))
    }

    /// Number of subtrees requested.
    #[must_use]
    pub fn subtree_count(&self) -> usize {
        self.subtree_roots.len()
    }

    /// Check if this is an empty request (no subtrees).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subtree_roots.is_empty()
    }

    /// Check if request is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// resource exhaustion attacks.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.subtree_roots.len() <= MAX_SUBTREES_PER_REQUEST
    }
}

/// Response containing prefetched subtrees.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SubtreePrefetchResponse {
    /// Fetched subtrees.
    ///
    /// Limited to MAX_SUBTREES_PER_REQUEST entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub subtrees: Vec<SubtreeData>,

    /// Subtree roots that were not found.
    ///
    /// Limited to MAX_SUBTREES_PER_REQUEST entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub not_found: Vec<[u8; 32]>,
}

impl SubtreePrefetchResponse {
    /// Create a new response.
    #[must_use]
    pub fn new(subtrees: Vec<SubtreeData>, not_found: Vec<[u8; 32]>) -> Self {
        Self {
            subtrees,
            not_found,
        }
    }

    /// Create a response with no missing subtrees.
    #[must_use]
    pub fn complete(subtrees: Vec<SubtreeData>) -> Self {
        Self {
            subtrees,
            not_found: vec![],
        }
    }

    /// Create an empty/not-found response.
    #[must_use]
    pub fn not_found(roots: Vec<[u8; 32]>) -> Self {
        Self {
            subtrees: vec![],
            not_found: roots,
        }
    }

    /// Check if all requested subtrees were found.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.not_found.is_empty()
    }

    /// Check if response is empty (no subtrees and none not found).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subtrees.is_empty() && self.not_found.is_empty()
    }

    /// Total number of entities across all subtrees.
    ///
    /// Uses saturating arithmetic to prevent overflow from malicious input.
    #[must_use]
    pub fn total_entity_count(&self) -> usize {
        self.subtrees
            .iter()
            .fold(0usize, |acc, s| acc.saturating_add(s.entity_count()))
    }

    /// Number of subtrees returned.
    #[must_use]
    pub fn subtree_count(&self) -> usize {
        self.subtrees.len()
    }

    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// resource exhaustion attacks. Validates both response size, total
    /// entity count, and all contained subtrees.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check subtree count limits
        if self.subtrees.len() > MAX_SUBTREES_PER_REQUEST {
            return false;
        }
        if self.not_found.len() > MAX_SUBTREES_PER_REQUEST {
            return false;
        }

        // Check total entity count (even if individual subtrees are valid)
        if self.total_entity_count() > MAX_TOTAL_ENTITIES {
            return false;
        }

        // Validate all subtrees
        self.subtrees.iter().all(SubtreeData::is_valid)
    }
}

// =============================================================================
// SubtreeData
// =============================================================================

/// Data for a single subtree.
///
/// Contains all entities within the subtree for bulk CRDT merge.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SubtreeData {
    /// Root ID of this subtree.
    pub root_id: [u8; 32],

    /// Root hash of this subtree (for verification).
    pub root_hash: [u8; 32],

    /// All entities in this subtree (leaves only).
    /// Includes full data and metadata for CRDT merge.
    ///
    /// Limited to MAX_ENTITIES_PER_SUBTREE entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub entities: Vec<TreeLeafData>,

    /// Depth of this subtree (how many levels were traversed).
    pub depth: usize,

    /// Whether the subtree was truncated due to depth limit.
    pub truncated: bool,
}

impl SubtreeData {
    /// Create subtree data.
    #[must_use]
    pub fn new(
        root_id: [u8; 32],
        root_hash: [u8; 32],
        entities: Vec<TreeLeafData>,
        depth: usize,
    ) -> Self {
        Self {
            root_id,
            root_hash,
            entities,
            depth,
            truncated: false,
        }
    }

    /// Create truncated subtree data (depth limit reached).
    #[must_use]
    pub fn truncated(
        root_id: [u8; 32],
        root_hash: [u8; 32],
        entities: Vec<TreeLeafData>,
        depth: usize,
    ) -> Self {
        Self {
            root_id,
            root_hash,
            entities,
            depth,
            truncated: true,
        }
    }

    /// Number of entities in this subtree.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Check if subtree is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Check if more data exists beyond depth limit.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Check if subtree data is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// resource exhaustion attacks. Validates entity count, depth, and all
    /// contained leaf data.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check entity count limit
        if self.entities.len() > MAX_ENTITIES_PER_SUBTREE {
            return false;
        }

        // Check depth limit
        if self.depth > MAX_SUBTREE_DEPTH {
            return false;
        }

        // Validate all leaf data
        self.entities.iter().all(TreeLeafData::is_valid)
    }
}

// =============================================================================
// Heuristic Function
// =============================================================================

/// Compare HashComparison vs SubtreePrefetch for a given scenario.
///
/// Returns true if SubtreePrefetch is likely more efficient.
#[must_use]
pub fn should_use_subtree_prefetch(
    tree_depth: usize,
    divergence_ratio: f64,
    estimated_differing_subtrees: usize,
) -> bool {
    // SubtreePrefetch is better when:
    // - Tree is deep (> 3 levels)
    // - Divergence is moderate (< 20%)
    // - Changes are clustered (few differing subtrees)

    let deep_tree = tree_depth > 3;
    let moderate_divergence = divergence_ratio < 0.20;
    let clustered_changes = estimated_differing_subtrees <= 5;

    deep_tree && moderate_divergence && clustered_changes
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

    fn make_leaf(key: u8, value: Vec<u8>) -> TreeLeafData {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [key; 32]);
        TreeLeafData::new([key; 32], value, metadata)
    }

    fn make_subtree(root_id: u8, entities: Vec<TreeLeafData>, depth: usize) -> SubtreeData {
        SubtreeData::new([root_id; 32], [root_id + 100; 32], entities, depth)
    }

    // =========================================================================
    // SubtreePrefetchRequest Tests
    // =========================================================================

    #[test]
    fn test_subtree_prefetch_request_new() {
        let roots = vec![[1u8; 32], [2u8; 32]];
        let request = SubtreePrefetchRequest::new(roots.clone());

        assert_eq!(request.subtree_roots, roots);
        assert_eq!(request.max_depth, Some(DEFAULT_SUBTREE_MAX_DEPTH));
        assert_eq!(request.subtree_count(), 2);
        assert!(!request.is_empty());
        assert!(request.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_request_empty() {
        let request = SubtreePrefetchRequest::new(vec![]);

        assert!(request.is_empty());
        assert_eq!(request.subtree_count(), 0);
        assert!(request.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_request_with_depth() {
        let roots = vec![[1u8; 32]];
        let request = SubtreePrefetchRequest::with_depth(roots, 10);

        assert_eq!(request.max_depth, Some(10));
        assert_eq!(request.depth(), Some(10));
    }

    #[test]
    fn test_subtree_prefetch_request_with_zero_depth() {
        let roots = vec![[1u8; 32]];
        let request = SubtreePrefetchRequest::with_depth(roots, 0);

        assert_eq!(request.max_depth, Some(0));
        assert_eq!(request.depth(), Some(0));
    }

    #[test]
    fn test_subtree_prefetch_request_depth_clamping() {
        // Test that depth is clamped during construction
        let request = SubtreePrefetchRequest::with_depth(vec![[1u8; 32]], MAX_SUBTREE_DEPTH);
        assert_eq!(request.max_depth, Some(MAX_SUBTREE_DEPTH));

        let excessive =
            SubtreePrefetchRequest::with_depth(vec![[1u8; 32]], MAX_SUBTREE_DEPTH + 100);
        assert_eq!(excessive.max_depth, Some(MAX_SUBTREE_DEPTH));
    }

    #[test]
    fn test_subtree_prefetch_request_depth_accessor() {
        // Test that depth accessor clamps even if raw field was set to larger value
        let mut request = SubtreePrefetchRequest::new(vec![[1u8; 32]]);
        request.max_depth = Some(usize::MAX); // Simulate untrusted deserialization

        assert_eq!(request.depth(), Some(MAX_SUBTREE_DEPTH));

        // Test None case
        let request_none = SubtreePrefetchRequest::unlimited_depth(vec![[1u8; 32]]);
        assert_eq!(request_none.depth(), None);
    }

    #[test]
    fn test_subtree_prefetch_request_unlimited() {
        let roots = vec![[1u8; 32]];
        let request = SubtreePrefetchRequest::unlimited_depth(roots);

        assert!(request.max_depth.is_none());
        assert!(request.depth().is_none());
    }

    #[test]
    fn test_subtree_prefetch_request_roundtrip() {
        let request = SubtreePrefetchRequest::with_depth(vec![[1u8; 32], [2u8; 32]], 7);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: SubtreePrefetchRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    #[test]
    fn test_subtree_prefetch_request_validation() {
        // Valid request at limit
        let roots: Vec<[u8; 32]> = (0..MAX_SUBTREES_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let at_limit = SubtreePrefetchRequest::new(roots);
        assert!(at_limit.is_valid());

        // Invalid request over limit
        let roots: Vec<[u8; 32]> = (0..=MAX_SUBTREES_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let over_limit = SubtreePrefetchRequest::new(roots);
        assert!(!over_limit.is_valid());
    }

    // =========================================================================
    // SubtreeData Tests
    // =========================================================================

    #[test]
    fn test_subtree_data_new() {
        let leaf = make_leaf(1, vec![1, 2, 3]);
        let subtree = SubtreeData::new([10; 32], [11; 32], vec![leaf], 3);

        assert_eq!(subtree.root_id, [10; 32]);
        assert_eq!(subtree.root_hash, [11; 32]);
        assert_eq!(subtree.entity_count(), 1);
        assert_eq!(subtree.depth, 3);
        assert!(!subtree.is_truncated());
        assert!(!subtree.is_empty());
        assert!(subtree.is_valid());
    }

    #[test]
    fn test_subtree_data_truncated() {
        let leaf = make_leaf(2, vec![4, 5, 6]);
        let subtree = SubtreeData::truncated([20; 32], [21; 32], vec![leaf], 5);

        assert!(subtree.is_truncated());
        assert_eq!(subtree.depth, 5);
        assert!(subtree.is_valid());
    }

    #[test]
    fn test_subtree_data_empty() {
        let subtree = SubtreeData::new([30; 32], [31; 32], vec![], 1);

        assert!(subtree.is_empty());
        assert_eq!(subtree.entity_count(), 0);
        assert!(subtree.is_valid());
    }

    #[test]
    fn test_subtree_data_zero_depth() {
        let leaf = make_leaf(1, vec![1, 2, 3]);
        let subtree = SubtreeData::new([10; 32], [11; 32], vec![leaf], 0);

        assert_eq!(subtree.depth, 0);
        assert!(subtree.is_valid());
    }

    #[test]
    fn test_subtree_data_multiple_entities() {
        let leaves = vec![
            make_leaf(1, vec![1, 2, 3]),
            make_leaf(2, vec![4, 5, 6]),
            make_leaf(3, vec![7, 8, 9]),
        ];
        let subtree = SubtreeData::new([10; 32], [11; 32], leaves, 3);

        assert_eq!(subtree.entity_count(), 3);
        assert!(!subtree.is_empty());
        assert!(subtree.is_valid());
    }

    #[test]
    fn test_subtree_data_roundtrip() {
        let leaf = make_leaf(3, vec![7, 8, 9]);
        let subtree = SubtreeData::truncated([40; 32], [41; 32], vec![leaf], 4);

        let encoded = borsh::to_vec(&subtree).expect("serialize");
        let decoded: SubtreeData = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(subtree, decoded);
    }

    #[test]
    fn test_subtree_data_validation() {
        // Valid subtree with valid leaf
        let valid_leaf = make_leaf(1, vec![1, 2, 3]);
        let valid = SubtreeData::new([1; 32], [2; 32], vec![valid_leaf], 2);
        assert!(valid.is_valid());

        // Invalid subtree with oversized leaf value
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let invalid_leaf = TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid = SubtreeData::new([1; 32], [2; 32], vec![invalid_leaf], 2);
        assert!(!invalid.is_valid());
    }

    // =========================================================================
    // SubtreePrefetchResponse Tests
    // =========================================================================

    #[test]
    fn test_subtree_prefetch_response_complete() {
        let leaf = make_leaf(1, vec![1, 2, 3]);
        let subtree = make_subtree(10, vec![leaf], 2);

        let response = SubtreePrefetchResponse::complete(vec![subtree]);

        assert!(response.is_complete());
        assert!(!response.is_empty());
        assert_eq!(response.subtree_count(), 1);
        assert_eq!(response.total_entity_count(), 1);
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_response_not_found() {
        let response = SubtreePrefetchResponse::not_found(vec![[1u8; 32], [2u8; 32]]);

        assert!(!response.is_complete());
        assert!(!response.is_empty());
        assert_eq!(response.subtree_count(), 0);
        assert_eq!(response.not_found.len(), 2);
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_response_empty() {
        let response = SubtreePrefetchResponse::new(vec![], vec![]);

        assert!(response.is_complete());
        assert!(response.is_empty());
        assert_eq!(response.subtree_count(), 0);
        assert_eq!(response.total_entity_count(), 0);
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_response_partial() {
        let leaf1 = make_leaf(1, vec![1, 2]);
        let leaf2 = make_leaf(2, vec![3, 4]);

        let subtree1 = make_subtree(10, vec![leaf1], 2);
        let subtree2 = make_subtree(20, vec![leaf2], 3);

        let response = SubtreePrefetchResponse::new(
            vec![subtree1, subtree2],
            vec![[30u8; 32]], // One not found
        );

        assert!(!response.is_complete());
        assert!(!response.is_empty());
        assert_eq!(response.subtree_count(), 2);
        assert_eq!(response.total_entity_count(), 2);
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_response_with_empty_subtrees() {
        // Some subtrees have entities, some don't
        let leaf = make_leaf(1, vec![1, 2, 3]);
        let populated = make_subtree(10, vec![leaf], 2);
        let empty = make_subtree(20, vec![], 1);

        let response = SubtreePrefetchResponse::complete(vec![populated, empty]);

        assert!(response.is_complete());
        assert_eq!(response.subtree_count(), 2);
        assert_eq!(response.total_entity_count(), 1); // Only one entity across all subtrees
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_prefetch_response_total_entity_count_multiple() {
        let subtree1 = make_subtree(1, vec![make_leaf(1, vec![1]), make_leaf(2, vec![2])], 2);
        let subtree2 = make_subtree(
            2,
            vec![
                make_leaf(3, vec![3]),
                make_leaf(4, vec![4]),
                make_leaf(5, vec![5]),
            ],
            3,
        );
        let subtree3 = make_subtree(3, vec![], 1); // Empty

        let response = SubtreePrefetchResponse::complete(vec![subtree1, subtree2, subtree3]);

        assert_eq!(response.subtree_count(), 3);
        assert_eq!(response.total_entity_count(), 5); // 2 + 3 + 0
    }

    #[test]
    fn test_subtree_prefetch_response_roundtrip() {
        let leaf = make_leaf(4, vec![10, 11, 12]);
        let subtree = make_subtree(50, vec![leaf], 2);

        let response = SubtreePrefetchResponse::new(vec![subtree], vec![[60u8; 32]]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: SubtreePrefetchResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_subtree_prefetch_response_validation() {
        // Valid response at subtree limit
        let subtrees: Vec<SubtreeData> = (0..MAX_SUBTREES_PER_REQUEST)
            .map(|i| make_subtree(i as u8, vec![], 1))
            .collect();
        let at_limit = SubtreePrefetchResponse::complete(subtrees);
        assert!(at_limit.is_valid());

        // Invalid response over subtree limit
        let subtrees: Vec<SubtreeData> = (0..=MAX_SUBTREES_PER_REQUEST)
            .map(|i| make_subtree(i as u8, vec![], 1))
            .collect();
        let over_limit = SubtreePrefetchResponse::complete(subtrees);
        assert!(!over_limit.is_valid());

        // Invalid response over not_found limit
        let not_found: Vec<[u8; 32]> = (0..=MAX_SUBTREES_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let over_not_found = SubtreePrefetchResponse::not_found(not_found);
        assert!(!over_not_found.is_valid());

        // Invalid response with invalid subtree
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let invalid_leaf = TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_subtree = SubtreeData::new([1; 32], [2; 32], vec![invalid_leaf], 2);
        let response_with_invalid = SubtreePrefetchResponse::complete(vec![invalid_subtree]);
        assert!(!response_with_invalid.is_valid());
    }

    // =========================================================================
    // Heuristic Function Tests
    // =========================================================================

    #[test]
    fn test_should_use_subtree_prefetch_basic() {
        // Deep tree, moderate divergence, clustered - YES
        assert!(should_use_subtree_prefetch(5, 0.10, 3));

        // Deep tree, high divergence - NO
        assert!(!should_use_subtree_prefetch(5, 0.30, 3));

        // Shallow tree - NO
        assert!(!should_use_subtree_prefetch(2, 0.10, 3));

        // Many differing subtrees (scattered) - NO
        assert!(!should_use_subtree_prefetch(5, 0.10, 10));
    }

    #[test]
    fn test_should_use_subtree_prefetch_boundary_conditions() {
        // Exactly at depth threshold (depth > 3, not >=)
        assert!(!should_use_subtree_prefetch(3, 0.10, 3)); // depth = 3, not > 3
        assert!(should_use_subtree_prefetch(4, 0.10, 3)); // depth = 4, > 3

        // Exactly at divergence threshold (< 0.20, not <=)
        assert!(!should_use_subtree_prefetch(5, 0.20, 3)); // divergence = 0.20, not < 0.20
        assert!(should_use_subtree_prefetch(5, 0.19, 3)); // divergence = 0.19, < 0.20
        assert!(should_use_subtree_prefetch(5, 0.199999, 3)); // just under

        // Exactly at subtree threshold (<= 5)
        assert!(should_use_subtree_prefetch(5, 0.10, 5)); // subtrees = 5, <= 5
        assert!(!should_use_subtree_prefetch(5, 0.10, 6)); // subtrees = 6, > 5
    }

    #[test]
    fn test_should_use_subtree_prefetch_edge_cases() {
        // Zero values
        assert!(!should_use_subtree_prefetch(0, 0.10, 3)); // zero depth
        assert!(should_use_subtree_prefetch(5, 0.0, 3)); // zero divergence
        assert!(should_use_subtree_prefetch(5, 0.10, 0)); // zero subtrees

        // Very large values
        assert!(should_use_subtree_prefetch(1000, 0.10, 3)); // very deep tree
        assert!(!should_use_subtree_prefetch(5, 1.0, 3)); // 100% divergence
        assert!(!should_use_subtree_prefetch(5, 10.0, 3)); // >100% divergence (edge case)
        assert!(!should_use_subtree_prefetch(5, 0.10, 1000)); // many subtrees

        // All conditions fail
        assert!(!should_use_subtree_prefetch(2, 0.50, 10));

        // All conditions pass with extreme values
        assert!(should_use_subtree_prefetch(100, 0.001, 1));
    }

    #[test]
    fn test_should_use_subtree_prefetch_typical_scenarios() {
        // Scenario 1: Large, deep tree with clustered changes (ideal for subtree prefetch)
        assert!(should_use_subtree_prefetch(10, 0.05, 2));

        // Scenario 2: Shallow config tree (not suitable)
        assert!(!should_use_subtree_prefetch(2, 0.05, 2));

        // Scenario 3: Deep tree but scattered changes (HashComparison better)
        assert!(!should_use_subtree_prefetch(10, 0.05, 20));

        // Scenario 4: Deep tree but high divergence (full sync better)
        assert!(!should_use_subtree_prefetch(10, 0.60, 2));

        // Scenario 5: Medium tree with moderate changes (borderline)
        assert!(should_use_subtree_prefetch(4, 0.15, 5));
    }

    // =========================================================================
    // Validation Edge Case Tests
    // =========================================================================

    #[test]
    fn test_subtree_data_validation_entity_limit() {
        // At entity limit - should be valid
        let leaves: Vec<TreeLeafData> = (0..MAX_ENTITIES_PER_SUBTREE)
            .map(|i| make_leaf(i as u8, vec![i as u8]))
            .collect();
        let at_limit = SubtreeData::new([1; 32], [2; 32], leaves, 5);
        assert!(at_limit.is_valid());

        // Over entity limit - should be invalid
        let leaves: Vec<TreeLeafData> = (0..=MAX_ENTITIES_PER_SUBTREE)
            .map(|i| make_leaf(i as u8, vec![i as u8]))
            .collect();
        let over_limit = SubtreeData::new([1; 32], [2; 32], leaves, 5);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_subtree_data_validation_depth_limit() {
        let leaf = make_leaf(1, vec![1, 2, 3]);

        // At depth limit - should be valid
        let at_limit = SubtreeData::new([1; 32], [2; 32], vec![leaf.clone()], MAX_SUBTREE_DEPTH);
        assert!(at_limit.is_valid());

        // Over depth limit - should be invalid
        let over_limit = SubtreeData::new([1; 32], [2; 32], vec![leaf], MAX_SUBTREE_DEPTH + 1);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_subtree_response_validation_total_entity_limit() {
        use super::MAX_TOTAL_ENTITIES;

        // Create subtrees that individually are valid but together exceed total limit
        // Each subtree has MAX_ENTITIES_PER_SUBTREE, and we create enough to exceed MAX_TOTAL_ENTITIES
        let entities_per_subtree = 1000;
        let num_subtrees = (MAX_TOTAL_ENTITIES / entities_per_subtree) + 1;

        let subtrees: Vec<SubtreeData> = (0..num_subtrees)
            .map(|i| {
                let leaves: Vec<TreeLeafData> = (0..entities_per_subtree)
                    .map(|j| make_leaf((i * 100 + j) as u8, vec![(i * 100 + j) as u8]))
                    .collect();
                SubtreeData::new([i as u8; 32], [(i + 100) as u8; 32], leaves, 5)
            })
            .collect();

        let response = SubtreePrefetchResponse::complete(subtrees);
        assert!(!response.is_valid()); // Should be invalid due to total entity count
    }

    // =========================================================================
    // Security / Exploit Tests
    // =========================================================================

    #[test]
    fn test_subtree_request_memory_exhaustion_prevention() {
        // Request with maximum allowed roots should be valid
        let roots: Vec<[u8; 32]> = (0..MAX_SUBTREES_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let valid = SubtreePrefetchRequest::new(roots);
        assert!(valid.is_valid());

        // Request exceeding limit should be invalid
        let roots: Vec<[u8; 32]> = (0..=MAX_SUBTREES_PER_REQUEST)
            .map(|i| [i as u8; 32])
            .collect();
        let invalid = SubtreePrefetchRequest::new(roots);
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_subtree_depth_exhaustion_prevention() {
        // Attempt to request extremely deep traversal should be clamped
        let request = SubtreePrefetchRequest::with_depth(vec![[1u8; 32]], usize::MAX);
        assert_eq!(request.max_depth, Some(MAX_SUBTREE_DEPTH));

        // Accessor should also clamp even if raw field was set to larger value
        let mut request = SubtreePrefetchRequest::new(vec![[1u8; 32]]);
        request.max_depth = Some(usize::MAX); // Simulate untrusted deserialization

        assert_eq!(request.depth(), Some(MAX_SUBTREE_DEPTH));
    }

    #[test]
    fn test_subtree_total_entity_count_overflow_prevention() {
        // Create a response with subtrees containing many entities
        // to test that total_entity_count uses saturating arithmetic
        let leaf = make_leaf(1, vec![1, 2, 3]);
        let subtree = SubtreeData::new([1; 32], [2; 32], vec![leaf], 2);

        let response = SubtreePrefetchResponse::complete(vec![subtree]);

        // Should not panic or overflow
        let count = response.total_entity_count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_subtree_cross_validation_consistency() {
        // Verify that individual subtree validation is enforced in response validation
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let oversized_leaf =
            TreeLeafData::new([1; 32], vec![0u8; MAX_LEAF_VALUE_SIZE + 1], metadata);
        let invalid_subtree = SubtreeData::new([1; 32], [2; 32], vec![oversized_leaf], 2);

        // Invalid subtree by itself
        assert!(!invalid_subtree.is_valid());

        // Response containing invalid subtree should also be invalid
        let response = SubtreePrefetchResponse::complete(vec![invalid_subtree]);
        assert!(!response.is_valid());
    }

    #[test]
    fn test_subtree_special_values() {
        // All zeros
        let zeros_subtree = SubtreeData::new([0u8; 32], [0u8; 32], vec![], 0);
        assert!(zeros_subtree.is_valid());

        // All ones
        let ones_subtree = SubtreeData::new([0xFF; 32], [0xFF; 32], vec![], MAX_SUBTREE_DEPTH);
        assert!(ones_subtree.is_valid());

        // Request with all-zeros roots
        let request = SubtreePrefetchRequest::new(vec![[0u8; 32]]);
        assert!(request.is_valid());

        // Request with all-ones roots
        let request = SubtreePrefetchRequest::new(vec![[0xFF; 32]]);
        assert!(request.is_valid());
    }

    #[test]
    fn test_subtree_serialization_roundtrip_with_edge_values() {
        // Test roundtrip with boundary values
        let leaf = make_leaf(0xFF, vec![0xFF; 100]);
        let subtree = SubtreeData::truncated([0xFF; 32], [0u8; 32], vec![leaf], MAX_SUBTREE_DEPTH);

        let encoded = borsh::to_vec(&subtree).expect("serialize");
        let decoded: SubtreeData = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(subtree, decoded);
        assert!(decoded.is_valid());
        assert!(decoded.is_truncated());
        assert_eq!(decoded.depth, MAX_SUBTREE_DEPTH);
    }

    #[test]
    fn test_subtree_response_all_not_found() {
        // Response where everything was not found (no subtrees returned)
        let not_found: Vec<[u8; 32]> = (0..50).map(|i| [i as u8; 32]).collect();
        let response = SubtreePrefetchResponse::not_found(not_found);

        assert!(!response.is_complete());
        assert!(!response.is_empty());
        assert_eq!(response.subtree_count(), 0);
        assert_eq!(response.total_entity_count(), 0);
        assert!(response.is_valid());
    }

    #[test]
    fn test_subtree_empty_entities_is_valid() {
        // Subtree with no entities (internal node with no leaves yet)
        let empty_subtree = SubtreeData::new([1; 32], [2; 32], vec![], 5);
        assert!(empty_subtree.is_valid());
        assert!(empty_subtree.is_empty());
        assert_eq!(empty_subtree.entity_count(), 0);
    }
}
