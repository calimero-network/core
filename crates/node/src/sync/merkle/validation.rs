//! Request validation and range helpers for Merkle sync.

use calimero_node_primitives::sync::{NodeId, TreeParams};
use calimero_primitives::hash::Hash;

/// Result of validating a Merkle sync request.
#[derive(Debug)]
pub enum MerkleSyncRequestValidation {
    /// Request is valid, proceed with sync. Contains parsed cursor if provided.
    Valid {
        cursor: Option<calimero_node_primitives::sync::MerkleCursor>,
    },
    /// Context not found.
    ContextNotFound,
    /// Boundary root hash doesn't match current context state.
    BoundaryMismatch,
    /// Tree parameters are incompatible.
    IncompatibleParams,
    /// Resume cursor is too large.
    CursorTooLarge { size: usize, max: usize },
    /// Resume cursor failed to deserialize.
    CursorMalformed { error: String },
}

/// Validate a Merkle sync request (pure function).
///
/// This validates all request parameters without performing I/O.
/// On success, returns the parsed cursor (if provided) to avoid double deserialization.
pub fn validate_merkle_sync_request(
    context_root_hash: Option<Hash>,
    boundary_root_hash: Hash,
    tree_params: &TreeParams,
    resume_cursor: Option<&[u8]>,
) -> MerkleSyncRequestValidation {
    // Check context exists
    let Some(current_root) = context_root_hash else {
        return MerkleSyncRequestValidation::ContextNotFound;
    };

    // Check boundary matches
    if current_root != boundary_root_hash {
        return MerkleSyncRequestValidation::BoundaryMismatch;
    }

    // Check tree params compatibility
    let our_params = TreeParams::default();
    if !our_params.is_compatible(tree_params) {
        return MerkleSyncRequestValidation::IncompatibleParams;
    }

    // Validate and parse resume cursor if provided
    let parsed_cursor = if let Some(cursor_bytes) = resume_cursor {
        if cursor_bytes.len() > calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE {
            return MerkleSyncRequestValidation::CursorTooLarge {
                size: cursor_bytes.len(),
                max: calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE,
            };
        }

        match borsh::from_slice::<calimero_node_primitives::sync::MerkleCursor>(cursor_bytes) {
            Ok(cursor) => Some(cursor),
            Err(e) => {
                return MerkleSyncRequestValidation::CursorMalformed {
                    error: e.to_string(),
                };
            }
        }
    } else {
        None
    };

    MerkleSyncRequestValidation::Valid {
        cursor: parsed_cursor,
    }
}

/// Result of parsing a snapshot boundary response for Merkle sync.
#[derive(Debug)]
pub enum BoundaryParseResult {
    /// Successfully parsed, Merkle sync is supported.
    MerkleSupported(MerkleSyncBoundary),
    /// Peer doesn't support Merkle sync (no tree_params).
    NoTreeParams,
    /// Peer doesn't support Merkle sync (no merkle_root_hash).
    NoMerkleRootHash,
    /// Tree params are incompatible.
    IncompatibleParams,
}

/// Parse a snapshot boundary response to check for Merkle sync support (pure function).
pub fn parse_boundary_for_merkle(
    boundary_root_hash: Hash,
    dag_heads: Vec<[u8; 32]>,
    tree_params: Option<TreeParams>,
    merkle_root_hash: Option<Hash>,
) -> BoundaryParseResult {
    let Some(tree_params) = tree_params else {
        return BoundaryParseResult::NoTreeParams;
    };

    let Some(merkle_root_hash) = merkle_root_hash else {
        return BoundaryParseResult::NoMerkleRootHash;
    };

    // Verify params are compatible
    let our_params = TreeParams::default();
    if !our_params.is_compatible(&tree_params) {
        return BoundaryParseResult::IncompatibleParams;
    }

    BoundaryParseResult::MerkleSupported(MerkleSyncBoundary {
        boundary_root_hash,
        tree_params,
        merkle_root_hash,
        dag_heads,
    })
}

/// Check if a key falls within any of the given sorted ranges (pure function).
///
/// Ranges must be sorted by start key for binary search to work correctly.
/// This is O(log M) where M is the number of ranges.
pub fn key_in_sorted_ranges(key: &[u8; 32], sorted_ranges: &[([u8; 32], [u8; 32])]) -> bool {
    if sorted_ranges.is_empty() {
        return false;
    }

    match sorted_ranges.binary_search_by(|(start, _)| start.cmp(key)) {
        Ok(idx) => {
            // Exact match on start_key - check if within this range
            *key <= sorted_ranges[idx].1
        }
        Err(0) => false, // key is before all ranges
        Err(idx) => {
            // Check the range just before where key would be inserted
            let (start, end) = &sorted_ranges[idx - 1];
            *key >= *start && *key <= *end
        }
    }
}

/// Sort ranges by start key for use with `key_in_sorted_ranges`.
pub fn sort_ranges(ranges: &[([u8; 32], [u8; 32])]) -> Vec<([u8; 32], [u8; 32])> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted
}

/// Boundary information for Merkle sync.
#[derive(Debug, Clone)]
pub struct MerkleSyncBoundary {
    pub boundary_root_hash: Hash,
    pub tree_params: TreeParams,
    pub merkle_root_hash: Hash,
    /// DAG heads at the boundary. Reserved for future use (e.g., post-sync DAG verification).
    #[allow(dead_code)]
    pub dag_heads: Vec<[u8; 32]>,
}

/// Result of a Merkle sync operation.
#[derive(Debug)]
pub struct MerkleSyncResult {
    pub chunks_transferred: usize,
    pub records_applied: usize,
}

/// Create a resume cursor from current traversal state.
///
/// This can be used to persist the traversal state for later resumption
/// if the sync is interrupted (e.g., connection drop, timeout).
///
/// The `covered_ranges` parameter is critical for correct orphan key deletion
/// on resume - without it, keys processed in a previous run could be incorrectly deleted.
///
/// Returns `None` if the cursor would exceed the size limit (64 KiB),
/// in which case the caller should fall back to snapshot sync.
#[allow(dead_code)] // Public API for resumable sync - will be used by persistence layer
pub fn create_resume_cursor(
    pending_nodes: &[NodeId],
    pending_leaves: &[u64],
    covered_ranges: &[([u8; 32], [u8; 32])],
) -> Option<calimero_node_primitives::sync::MerkleCursor> {
    let cursor = calimero_node_primitives::sync::MerkleCursor {
        pending_nodes: pending_nodes.to_vec(),
        pending_leaves: pending_leaves.to_vec(),
        covered_ranges: covered_ranges.to_vec(),
    };

    if cursor.exceeds_size_limit() {
        None
    } else {
        Some(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Tests for key_in_sorted_ranges
    // =========================================================================

    #[test]
    fn test_key_in_sorted_ranges_empty() {
        let key = [5u8; 32];
        let ranges: Vec<([u8; 32], [u8; 32])> = vec![];
        assert!(!key_in_sorted_ranges(&key, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_exact_start() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];
        assert!(key_in_sorted_ranges(&start, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_exact_end() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];
        assert!(key_in_sorted_ranges(&end, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_middle() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];

        let mut key = [0u8; 32];
        key[0] = 15;
        assert!(key_in_sorted_ranges(&key, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_before() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];

        let mut key = [0u8; 32];
        key[0] = 5;
        assert!(!key_in_sorted_ranges(&key, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_after() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];

        let mut key = [0u8; 32];
        key[0] = 25;
        assert!(!key_in_sorted_ranges(&key, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_multiple_ranges() {
        let ranges = vec![
            ([0u8; 32], { let mut e = [0u8; 32]; e[0] = 10; e }),
            ({ let mut s = [0u8; 32]; s[0] = 20; s }, { let mut e = [0u8; 32]; e[0] = 30; e }),
            ({ let mut s = [0u8; 32]; s[0] = 50; s }, { let mut e = [0u8; 32]; e[0] = 60; e }),
        ];

        let mut key1 = [0u8; 32]; key1[0] = 5;
        assert!(key_in_sorted_ranges(&key1, &ranges));

        let mut key2 = [0u8; 32]; key2[0] = 25;
        assert!(key_in_sorted_ranges(&key2, &ranges));

        let mut key3 = [0u8; 32]; key3[0] = 55;
        assert!(key_in_sorted_ranges(&key3, &ranges));

        let mut key4 = [0u8; 32]; key4[0] = 15;
        assert!(!key_in_sorted_ranges(&key4, &ranges));

        let mut key5 = [0u8; 32]; key5[0] = 70;
        assert!(!key_in_sorted_ranges(&key5, &ranges));
    }

    #[test]
    fn test_sort_ranges() {
        let ranges = vec![
            ({ let mut s = [0u8; 32]; s[0] = 30; s }, { let mut e = [0u8; 32]; e[0] = 40; e }),
            ({ let mut s = [0u8; 32]; s[0] = 10; s }, { let mut e = [0u8; 32]; e[0] = 20; e }),
        ];

        let sorted = sort_ranges(&ranges);

        assert_eq!(sorted[0].0[0], 10);
        assert_eq!(sorted[1].0[0], 30);
    }

    // =========================================================================
    // Tests for validate_merkle_sync_request
    // =========================================================================

    #[test]
    fn test_validate_request_valid() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &params, None);

        assert!(matches!(result, MerkleSyncRequestValidation::Valid { cursor: None }));
    }

    #[test]
    fn test_validate_request_context_not_found() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(None, root_hash, &params, None);

        assert!(matches!(result, MerkleSyncRequestValidation::ContextNotFound));
    }

    #[test]
    fn test_validate_request_boundary_mismatch() {
        let current: Hash = [1u8; 32].into();
        let boundary: Hash = [2u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(Some(current), boundary, &params, None);

        assert!(matches!(result, MerkleSyncRequestValidation::BoundaryMismatch));
    }

    #[test]
    fn test_validate_request_cursor_too_large() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let large_cursor = vec![0u8; calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE + 1];

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &params, Some(&large_cursor));

        assert!(matches!(result, MerkleSyncRequestValidation::CursorTooLarge { .. }));
    }

    #[test]
    fn test_validate_request_cursor_malformed() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let malformed_cursor = vec![0xFF, 0xFF, 0xFF, 0xFF];

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &params, Some(&malformed_cursor));

        assert!(matches!(result, MerkleSyncRequestValidation::CursorMalformed { .. }));
    }

    #[test]
    fn test_validate_request_incompatible_params() {
        let root_hash: Hash = [1u8; 32].into();

        let incompatible_params = TreeParams {
            fanout: 999,
            ..Default::default()
        };

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &incompatible_params, None);

        assert!(matches!(result, MerkleSyncRequestValidation::IncompatibleParams));
    }

    #[test]
    fn test_validate_request_returns_parsed_cursor() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let cursor = calimero_node_primitives::sync::MerkleCursor {
            pending_nodes: vec![NodeId { level: 1, index: 0 }],
            pending_leaves: vec![1, 2, 3],
            covered_ranges: vec![],
        };
        let cursor_bytes = borsh::to_vec(&cursor).unwrap();

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &params, Some(&cursor_bytes));

        match result {
            MerkleSyncRequestValidation::Valid { cursor: Some(parsed) } => {
                assert_eq!(parsed.pending_nodes.len(), 1);
                assert_eq!(parsed.pending_leaves, vec![1, 2, 3]);
            }
            _ => panic!("Expected Valid with parsed cursor"),
        }
    }

    // =========================================================================
    // Tests for parse_boundary_for_merkle
    // =========================================================================

    #[test]
    fn test_parse_boundary_merkle_supported() {
        let boundary_root: Hash = [1u8; 32].into();
        let merkle_root: Hash = [2u8; 32].into();
        let params = TreeParams::default();
        let dag_heads = vec![[3u8; 32]];

        let result = parse_boundary_for_merkle(boundary_root, dag_heads.clone(), Some(params), Some(merkle_root));

        match result {
            BoundaryParseResult::MerkleSupported(boundary) => {
                assert_eq!(boundary.boundary_root_hash, boundary_root);
                assert_eq!(boundary.merkle_root_hash, merkle_root);
            }
            _ => panic!("Expected MerkleSupported"),
        }
    }

    #[test]
    fn test_parse_boundary_no_tree_params() {
        let boundary_root: Hash = [1u8; 32].into();
        let merkle_root: Hash = [2u8; 32].into();

        let result = parse_boundary_for_merkle(boundary_root, vec![], None, Some(merkle_root));

        assert!(matches!(result, BoundaryParseResult::NoTreeParams));
    }

    #[test]
    fn test_parse_boundary_no_merkle_root() {
        let boundary_root: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result = parse_boundary_for_merkle(boundary_root, vec![], Some(params), None);

        assert!(matches!(result, BoundaryParseResult::NoMerkleRootHash));
    }
}
