//! Pure state machine for Merkle tree traversal during sync.

use calimero_node_primitives::sync::{CompressedChunk, NodeDigest, NodeId, TreeParams};

use super::tree::{get_children_ids, MerkleTree};
use super::validation::MerkleSyncResult;

/// Pure state machine for Merkle tree traversal.
///
/// This struct holds all traversal state and provides pure methods for
/// computing the next action and processing responses. It contains no I/O
/// or side effects, making it fully unit-testable with synthetic inputs.
#[derive(Debug, Clone)]
pub struct MerkleTraversalState {
    /// Pending internal nodes to request hashes for.
    pub pending_nodes: Vec<NodeId>,
    /// Pending leaf indices to fetch chunks for.
    pub pending_leaves: Vec<u64>,
    /// Key ranges covered by the remote tree (for orphan deletion).
    pub covered_ranges: Vec<([u8; 32], [u8; 32])>,
    /// Number of chunks transferred so far.
    pub chunks_transferred: usize,
    /// Number of records applied so far.
    pub records_applied: usize,
    /// Tree parameters for computing children.
    tree_params: TreeParams,
    /// Page limit for batching requests.
    page_limit: usize,
}

/// Actions that the traversal state machine can request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraversalAction {
    /// Request node digests for the given node IDs.
    RequestNodes(Vec<NodeId>),
    /// Request leaf chunks for the given leaf indices.
    RequestLeaves(Vec<u64>),
    /// Traversal is complete.
    Done,
}

/// Result of processing a leaf reply - chunks to apply.
#[derive(Debug)]
#[allow(dead_code)] // Public API - fields accessed by callers
pub struct LeafReplyResult {
    /// Chunks that need to be applied to the store.
    pub chunks_to_apply: Vec<CompressedChunk>,
    /// Key ranges covered by these chunks (for tracking).
    pub covered_ranges: Vec<([u8; 32], [u8; 32])>,
}

impl MerkleTraversalState {
    /// Create a new traversal state starting from the tree root.
    pub fn new(root_id: NodeId, tree_params: TreeParams, page_limit: usize) -> Self {
        Self {
            pending_nodes: vec![root_id],
            pending_leaves: Vec::new(),
            covered_ranges: Vec::new(),
            chunks_transferred: 0,
            records_applied: 0,
            tree_params,
            page_limit,
        }
    }

    /// Create a traversal state from a resume cursor.
    pub fn from_cursor(
        cursor: calimero_node_primitives::sync::MerkleCursor,
        tree_params: TreeParams,
        page_limit: usize,
    ) -> Self {
        Self {
            pending_nodes: cursor.pending_nodes,
            pending_leaves: cursor.pending_leaves,
            covered_ranges: cursor.covered_ranges,
            chunks_transferred: 0,
            records_applied: 0,
            tree_params,
            page_limit,
        }
    }

    /// Get the next action to perform.
    ///
    /// Returns `Done` when traversal is complete.
    pub fn next_action(&mut self) -> TraversalAction {
        // Prioritize node requests over leaf requests (BFS)
        if !self.pending_nodes.is_empty() {
            let batch: Vec<NodeId> = self
                .pending_nodes
                .drain(..self.pending_nodes.len().min(self.page_limit))
                .collect();
            return TraversalAction::RequestNodes(batch);
        }

        if !self.pending_leaves.is_empty() {
            let batch: Vec<u64> = self
                .pending_leaves
                .drain(..self.pending_leaves.len().min(self.page_limit))
                .collect();
            return TraversalAction::RequestLeaves(batch);
        }

        TraversalAction::Done
    }

    /// Process a node reply by comparing remote digests with local tree.
    ///
    /// Updates internal state based on which nodes match vs mismatch.
    /// Returns the number of matching subtrees found.
    pub fn handle_node_reply(
        &mut self,
        local_tree: &MerkleTree,
        remote_digests: &[NodeDigest],
    ) -> usize {
        let mut matches = 0;

        for remote_digest in remote_digests {
            let local_hash = local_tree.get_node_hash(&remote_digest.id);

            match local_hash {
                Some(lh) if lh == remote_digest.hash => {
                    // Match - skip this subtree, but track its key range
                    if let Some(range) = local_tree.get_subtree_key_range(&remote_digest.id) {
                        self.covered_ranges.push(range);
                    }
                    matches += 1;
                }
                _ => {
                    // Mismatch - drill down
                    if remote_digest.id.level == 0 {
                        // Leaf node - queue for fetch
                        self.pending_leaves.push(remote_digest.id.index);
                    } else {
                        // Internal node - queue children
                        let children = get_children_ids(
                            &remote_digest.id,
                            remote_digest.child_count,
                            self.tree_params.fanout,
                        );
                        self.pending_nodes.extend(children);
                    }
                }
            }
        }

        matches
    }

    /// Process a leaf reply by extracting chunks to apply.
    ///
    /// Returns the chunks that need to be applied to the store.
    /// The caller is responsible for actually applying them and calling
    /// `record_chunk_applied` for each successful apply.
    pub fn handle_leaf_reply(&mut self, chunks: Vec<CompressedChunk>) -> LeafReplyResult {
        let mut chunks_to_apply = Vec::with_capacity(chunks.len());
        let mut covered_ranges = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            // Track the key range covered by this chunk
            if let (Ok(start), Ok(end)) = (
                chunk.start_key.as_slice().try_into(),
                chunk.end_key.as_slice().try_into(),
            ) {
                covered_ranges.push((start, end));
                self.covered_ranges.push((start, end));
            }
            chunks_to_apply.push(chunk);
        }

        LeafReplyResult {
            chunks_to_apply,
            covered_ranges,
        }
    }

    /// Record that a chunk was successfully applied with the given record count.
    ///
    /// Call this after each successful `apply_merkle_chunk` to accurately track
    /// chunks_transferred (only counting successfully applied chunks).
    pub fn record_chunk_applied(&mut self, records_applied: usize) {
        self.chunks_transferred += 1;
        self.records_applied += records_applied;
    }

    /// Check if traversal is complete.
    #[allow(dead_code)] // Public API for resumable sync
    pub fn is_done(&self) -> bool {
        self.pending_nodes.is_empty() && self.pending_leaves.is_empty()
    }

    /// Get the current result.
    pub fn result(&self) -> MerkleSyncResult {
        MerkleSyncResult {
            chunks_transferred: self.chunks_transferred,
            records_applied: self.records_applied,
        }
    }

    /// Get the covered ranges for orphan key deletion.
    pub fn covered_ranges(&self) -> &[([u8; 32], [u8; 32])] {
        &self.covered_ranges
    }

    /// Convert to a resume cursor for persistence.
    #[allow(dead_code)] // Public API for resumable sync
    pub fn to_cursor(&self) -> Option<calimero_node_primitives::sync::MerkleCursor> {
        super::validation::create_resume_cursor(
            &self.pending_nodes,
            &self.pending_leaves,
            &self.covered_ranges,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::merkle::tree::{build_internal_nodes, MerkleTree};
    use calimero_node_primitives::sync::SnapshotChunk;
    use calimero_primitives::hash::Hash;

    #[test]
    fn test_traversal_state_new() {
        let root_id = NodeId { level: 2, index: 0 };
        let params = TreeParams::default();
        let state = MerkleTraversalState::new(root_id, params, 100);

        assert_eq!(state.pending_nodes, vec![root_id]);
        assert!(state.pending_leaves.is_empty());
        assert!(state.covered_ranges.is_empty());
        assert_eq!(state.chunks_transferred, 0);
        assert_eq!(state.records_applied, 0);
        assert!(!state.is_done());
    }

    #[test]
    fn test_traversal_state_next_action_nodes_first() {
        let root_id = NodeId { level: 2, index: 0 };
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(root_id, params, 100);

        state.pending_leaves.push(0);
        state.pending_leaves.push(1);

        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => {
                assert_eq!(nodes, vec![root_id]);
            }
            _ => panic!("Expected RequestNodes"),
        }

        match state.next_action() {
            TraversalAction::RequestLeaves(leaves) => {
                assert_eq!(leaves, vec![0, 1]);
            }
            _ => panic!("Expected RequestLeaves"),
        }

        assert_eq!(state.next_action(), TraversalAction::Done);
        assert!(state.is_done());
    }

    #[test]
    fn test_traversal_state_batching() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params, 2);

        state.pending_nodes = vec![
            NodeId { level: 1, index: 0 },
            NodeId { level: 1, index: 1 },
            NodeId { level: 1, index: 2 },
        ];

        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => assert_eq!(nodes.len(), 2),
            _ => panic!("Expected RequestNodes"),
        }

        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => assert_eq!(nodes.len(), 1),
            _ => panic!("Expected RequestNodes"),
        }
    }

    fn make_test_tree(params: &TreeParams) -> MerkleTree {
        let leaf_hashes: Vec<Hash> = (0..4).map(|i| [i as u8; 32].into()).collect();
        let chunks: Vec<SnapshotChunk> = (0..4)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i;
                SnapshotChunk {
                    index: i as u64,
                    start_key: key.to_vec(),
                    end_key: key.to_vec(),
                    uncompressed_len: 100,
                    payload: vec![i; 100],
                }
            })
            .collect();

        let (node_hashes, root_hash, height) = build_internal_nodes(&leaf_hashes, 4);

        MerkleTree {
            params: params.clone(),
            chunks,
            leaf_hashes,
            node_hashes,
            root_hash,
            height,
        }
    }

    #[test]
    fn test_traversal_state_handle_node_reply_match() {
        let params = TreeParams::default();
        let local_tree = make_test_tree(&params);

        let mut state = MerkleTraversalState::new(local_tree.root_id(), params, 100);
        state.pending_nodes.clear();

        let remote_digests = vec![NodeDigest {
            id: local_tree.root_id(),
            hash: local_tree.root_hash,
            child_count: 4,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        assert_eq!(matches, 1);
        assert_eq!(state.covered_ranges.len(), 1);
        assert!(state.pending_nodes.is_empty());
        assert!(state.pending_leaves.is_empty());
    }

    #[test]
    fn test_traversal_state_handle_node_reply_mismatch_internal() {
        let params = TreeParams::default();
        let local_tree = make_test_tree(&params);

        let mut state = MerkleTraversalState::new(NodeId { level: 1, index: 0 }, params, 100);
        state.pending_nodes.clear();

        let remote_digests = vec![NodeDigest {
            id: NodeId { level: 1, index: 0 },
            hash: [99u8; 32].into(),
            child_count: 4,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        assert_eq!(matches, 0);
        assert!(state.covered_ranges.is_empty());
        assert_eq!(state.pending_nodes.len(), 4);
    }

    #[test]
    fn test_traversal_state_handle_node_reply_mismatch_leaf() {
        let params = TreeParams::default();
        let local_tree = make_test_tree(&params);

        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params, 100);
        state.pending_nodes.clear();

        let remote_digests = vec![NodeDigest {
            id: NodeId { level: 0, index: 2 },
            hash: [99u8; 32].into(),
            child_count: 0,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        assert_eq!(matches, 0);
        assert!(state.pending_nodes.is_empty());
        assert_eq!(state.pending_leaves, vec![2]);
    }

    #[test]
    fn test_traversal_state_handle_leaf_reply() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params, 100);
        state.pending_nodes.clear();

        let chunks = vec![
            CompressedChunk {
                index: 0,
                start_key: vec![0; 32],
                end_key: vec![10; 32],
                uncompressed_len: 100,
                compressed_payload: vec![1, 2, 3],
            },
            CompressedChunk {
                index: 1,
                start_key: vec![11; 32],
                end_key: vec![20; 32],
                uncompressed_len: 200,
                compressed_payload: vec![4, 5, 6],
            },
        ];

        let result = state.handle_leaf_reply(chunks);

        assert_eq!(result.chunks_to_apply.len(), 2);
        assert_eq!(result.covered_ranges.len(), 2);
        assert_eq!(state.chunks_transferred, 0);
        assert_eq!(state.covered_ranges.len(), 2);
    }

    #[test]
    fn test_traversal_state_record_chunk_applied() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params, 100);

        assert_eq!(state.chunks_transferred, 0);
        assert_eq!(state.records_applied, 0);

        state.record_chunk_applied(10);
        assert_eq!(state.chunks_transferred, 1);
        assert_eq!(state.records_applied, 10);

        state.record_chunk_applied(5);
        assert_eq!(state.chunks_transferred, 2);
        assert_eq!(state.records_applied, 15);
    }
}
