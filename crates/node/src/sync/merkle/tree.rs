//! Merkle tree construction and node hashing.

use std::collections::HashMap;

use calimero_node_primitives::sync::{NodeDigest, NodeId, SnapshotChunk, TreeParams};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use eyre::Result;
use sha2::{Digest, Sha256};

use crate::sync::snapshot::CanonicalRecord;

/// Check if a hash represents an empty Merkle tree (all zeros).
pub fn is_empty_tree_hash(hash: &Hash) -> bool {
    hash.is_zero()
}

/// A computed Merkle tree over snapshot chunks.
#[derive(Clone, Debug)]
pub struct MerkleTree {
    /// Tree parameters used to build this tree.
    pub params: TreeParams,
    /// Leaf chunks (level 0).
    pub chunks: Vec<SnapshotChunk>,
    /// Leaf hashes (level 0).
    pub leaf_hashes: Vec<Hash>,
    /// Internal node hashes, keyed by (level, index).
    /// Level 1 is the first internal level above leaves.
    pub node_hashes: HashMap<NodeId, Hash>,
    /// Root hash of the entire tree.
    pub root_hash: Hash,
    /// Total number of levels (including leaf level 0).
    pub height: u16,
}

impl MerkleTree {
    /// Build a Merkle tree from snapshot entries.
    pub fn build<L: calimero_store::layer::ReadLayer>(
        handle: &calimero_store::Handle<L>,
        context_id: ContextId,
        params: &TreeParams,
    ) -> Result<Self> {
        // Collect and sort entries (same as snapshot generation)
        let entries = collect_sorted_entries(handle, context_id)?;

        // Build chunks from entries
        let chunks = build_chunks(&entries, params)?;

        // Compute leaf hashes
        let leaf_hashes: Vec<Hash> = chunks.iter().map(compute_leaf_hash).collect();

        // Build internal nodes bottom-up
        let (node_hashes, root_hash, height) =
            build_internal_nodes(&leaf_hashes, params.fanout as usize);

        Ok(Self {
            params: params.clone(),
            chunks,
            leaf_hashes,
            node_hashes,
            root_hash,
            height,
        })
    }

    /// Get the hash for a node at the given level and index.
    pub fn get_node_hash(&self, id: &NodeId) -> Option<Hash> {
        if id.level == 0 {
            // Leaf level
            self.leaf_hashes.get(id.index as usize).copied()
        } else {
            // Internal node
            self.node_hashes.get(id).copied()
        }
    }

    /// Get the digest (hash + child count) for a node.
    pub fn get_node_digest(&self, id: &NodeId) -> Option<NodeDigest> {
        let hash = self.get_node_hash(id)?;
        let child_count = self.get_children(id).len() as u16;
        Some(NodeDigest {
            id: *id,
            hash,
            child_count,
        })
    }

    /// Count the number of nodes at a given level.
    fn nodes_at_level(&self, level: u16) -> u64 {
        if level == 0 {
            self.leaf_hashes.len() as u64
        } else {
            let fanout = self.params.fanout as u64;
            let below = self.nodes_at_level(level - 1);
            below.div_ceil(fanout)
        }
    }

    /// Get children IDs for an internal node.
    pub fn get_children(&self, id: &NodeId) -> Vec<NodeId> {
        if id.level == 0 {
            return vec![];
        }

        let fanout = self.params.fanout as u64;
        let child_level = id.level - 1;
        let first_child_idx = id.index * fanout;
        let total_at_child_level = self.nodes_at_level(child_level);
        let last_child_idx = ((id.index + 1) * fanout).min(total_at_child_level);

        (first_child_idx..last_child_idx)
            .map(|idx| NodeId {
                level: child_level,
                index: idx,
            })
            .collect()
    }

    /// Get the root node ID.
    pub fn root_id(&self) -> NodeId {
        NodeId {
            level: self.height - 1,
            index: 0,
        }
    }

    /// Get a leaf chunk by index.
    pub fn get_chunk(&self, index: u64) -> Option<&SnapshotChunk> {
        self.chunks.get(index as usize)
    }

    /// Get total number of leaves.
    pub fn leaf_count(&self) -> u64 {
        self.chunks.len() as u64
    }

    /// Get the range of leaf indices covered by a node (inclusive).
    ///
    /// For a leaf node (level 0), returns (index, index).
    /// For an internal node, computes the first and last leaf indices in its subtree.
    /// Returns indices clamped to actual leaf count to handle overflow safely.
    pub fn get_leaf_index_range(&self, id: &NodeId) -> (u64, u64) {
        if id.level == 0 {
            return (id.index, id.index);
        }

        let leaf_count = self.leaf_count();
        if leaf_count == 0 {
            return (0, 0);
        }

        let fanout = self.params.fanout as u64;

        // Use checked math to prevent overflow on very large trees
        // First leaf index: traverse down to the leftmost leaf
        let first_leaf = fanout
            .checked_pow(id.level as u32)
            .and_then(|scale| id.index.checked_mul(scale))
            .unwrap_or(leaf_count) // On overflow, clamp to leaf_count
            .min(leaf_count.saturating_sub(1));

        // Last leaf index: rightmost leaf in subtree, clamped to actual leaf count
        let last_leaf = fanout
            .checked_pow(id.level as u32)
            .and_then(|scale| (id.index + 1).checked_mul(scale))
            .and_then(|v| v.checked_sub(1))
            .unwrap_or(leaf_count.saturating_sub(1)) // On overflow, use max leaf
            .min(leaf_count.saturating_sub(1));

        (first_leaf, last_leaf)
    }

    /// Get the key range covered by a node's subtree.
    ///
    /// Returns None if the node covers no valid leaves.
    pub fn get_subtree_key_range(&self, id: &NodeId) -> Option<([u8; 32], [u8; 32])> {
        let (first_leaf, last_leaf) = self.get_leaf_index_range(id);

        let first_chunk = self.get_chunk(first_leaf)?;
        let last_chunk = self.get_chunk(last_leaf)?;

        let start: [u8; 32] = first_chunk.start_key.as_slice().try_into().ok()?;
        let end: [u8; 32] = last_chunk.end_key.as_slice().try_into().ok()?;

        Some((start, end))
    }
}

/// Collect and sort all entries for a context.
fn collect_sorted_entries<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
) -> Result<Vec<([u8; 32], Vec<u8>)>> {
    let mut iter = handle.iter_snapshot::<ContextStateKey>()?;
    let mut entries: Vec<([u8; 32], Vec<u8>)> = Vec::new();

    for (key_result, value_result) in iter.entries() {
        let key = key_result?;
        let value = value_result?;
        if key.context_id() == context_id {
            entries.push((key.state_key(), value.value.to_vec()));
        }
    }

    // Sort by state_key for canonical ordering
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Build chunks from sorted entries according to tree params.
pub fn build_chunks(
    entries: &[([u8; 32], Vec<u8>)],
    params: &TreeParams,
) -> Result<Vec<SnapshotChunk>> {
    if entries.is_empty() {
        return Ok(vec![]);
    }

    let mut chunks = Vec::new();
    let mut current_payload = Vec::new();
    let mut chunk_start_key: Option<[u8; 32]> = None;
    let mut chunk_end_key: [u8; 32] = [0; 32];

    for (key, value) in entries {
        let record = CanonicalRecord {
            key: *key,
            value: value.clone(),
        };
        let record_bytes = borsh::to_vec(&record)?;

        // Start new chunk if this record would exceed limit
        if !current_payload.is_empty()
            && (current_payload.len() + record_bytes.len()) as u32 > params.leaf_target_bytes
        {
            // Finalize current chunk
            chunks.push(SnapshotChunk {
                index: chunks.len() as u64,
                start_key: chunk_start_key.unwrap().to_vec(),
                end_key: chunk_end_key.to_vec(),
                uncompressed_len: current_payload.len() as u32,
                payload: std::mem::take(&mut current_payload),
            });
            chunk_start_key = None;
        }

        // Add record to current chunk
        if chunk_start_key.is_none() {
            chunk_start_key = Some(*key);
        }
        chunk_end_key = *key;
        current_payload.extend(record_bytes);
    }

    // Finalize last chunk
    if !current_payload.is_empty() {
        chunks.push(SnapshotChunk {
            index: chunks.len() as u64,
            start_key: chunk_start_key.unwrap().to_vec(),
            end_key: chunk_end_key.to_vec(),
            uncompressed_len: current_payload.len() as u32,
            payload: current_payload,
        });
    }

    Ok(chunks)
}

/// Compute the hash for a leaf chunk.
///
/// Formula: H("leaf" || index || payload_hash || uncompressed_len || start_key || end_key)
pub fn compute_leaf_hash(chunk: &SnapshotChunk) -> Hash {
    let payload_hash = Sha256::digest(&chunk.payload);

    let mut hasher = Sha256::new();
    hasher.update(b"leaf");
    hasher.update(chunk.index.to_le_bytes());
    hasher.update(payload_hash);
    hasher.update(chunk.uncompressed_len.to_le_bytes());
    hasher.update(&chunk.start_key);
    hasher.update(&chunk.end_key);

    let hash_bytes: [u8; 32] = hasher.finalize().into();
    hash_bytes.into()
}

/// Build internal nodes bottom-up and return (node_hashes, root_hash, height).
pub fn build_internal_nodes(
    leaf_hashes: &[Hash],
    fanout: usize,
) -> (HashMap<NodeId, Hash>, Hash, u16) {
    if leaf_hashes.is_empty() {
        // Empty tree
        let empty_hash: Hash = [0u8; 32].into();
        return (HashMap::new(), empty_hash, 1);
    }

    if leaf_hashes.len() == 1 {
        // Single leaf is the root
        return (HashMap::new(), leaf_hashes[0], 1);
    }

    let mut node_hashes = HashMap::new();
    let mut current_level_hashes: Vec<Hash> = leaf_hashes.to_vec();
    let mut level: u16 = 1;

    while current_level_hashes.len() > 1 {
        let mut next_level_hashes = Vec::new();

        for (node_idx, chunk) in current_level_hashes.chunks(fanout).enumerate() {
            let node_hash = compute_internal_node_hash(level, chunk);
            let node_id = NodeId {
                level,
                index: node_idx as u64,
            };
            node_hashes.insert(node_id, node_hash);
            next_level_hashes.push(node_hash);
        }

        current_level_hashes = next_level_hashes;
        level += 1;
    }

    let root_hash = current_level_hashes[0];
    (node_hashes, root_hash, level)
}

/// Compute hash for an internal node.
///
/// Formula: H("node" || level || child_hashes...)
pub fn compute_internal_node_hash(level: u16, child_hashes: &[Hash]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"node");
    hasher.update(level.to_le_bytes());
    for child_hash in child_hashes {
        hasher.update(child_hash.as_bytes());
    }

    let hash_bytes: [u8; 32] = hasher.finalize().into();
    hash_bytes.into()
}

/// Get children node IDs for an internal node.
pub fn get_children_ids(parent: &NodeId, child_count: u16, fanout: u16) -> Vec<NodeId> {
    let child_level = parent.level - 1;
    let first_child_idx = parent.index * fanout as u64;

    (0..child_count as u64)
        .map(|i| NodeId {
            level: child_level,
            index: first_child_idx + i,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_node_primitives::sync::SnapshotChunk;

    #[test]
    fn test_compute_leaf_hash_deterministic() {
        let chunk = SnapshotChunk {
            index: 0,
            start_key: vec![1; 32],
            end_key: vec![2; 32],
            uncompressed_len: 100,
            payload: vec![1, 2, 3, 4, 5],
        };

        let hash1 = compute_leaf_hash(&chunk);
        let hash2 = compute_leaf_hash(&chunk);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_internal_node_hash_deterministic() {
        let child_hashes: Vec<Hash> = vec![[1u8; 32].into(), [2u8; 32].into(), [3u8; 32].into()];

        let hash1 = compute_internal_node_hash(1, &child_hashes);
        let hash2 = compute_internal_node_hash(1, &child_hashes);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_build_internal_nodes_single_leaf() {
        let leaf_hashes: Vec<Hash> = vec![[1u8; 32].into()];
        let (node_hashes, root_hash, height) = build_internal_nodes(&leaf_hashes, 16);

        assert!(node_hashes.is_empty());
        assert_eq!(root_hash, leaf_hashes[0]);
        assert_eq!(height, 1);
    }

    #[test]
    fn test_build_internal_nodes_multiple_leaves() {
        let leaf_hashes: Vec<Hash> = (0..20).map(|i| [i as u8; 32].into()).collect();
        let (node_hashes, _root_hash, height) = build_internal_nodes(&leaf_hashes, 4);

        // With 20 leaves and fanout 4:
        // Level 0: 20 leaves (not stored in node_hashes)
        // Level 1: ceil(20/4) = 5 internal nodes
        // Level 2: ceil(5/4) = 2 internal nodes
        // Level 3: ceil(2/4) = 1 root
        // Height = 4 (levels 0, 1, 2, 3)
        assert_eq!(height, 4);
        assert!(!node_hashes.is_empty());
    }

    #[test]
    fn test_build_internal_nodes_empty() {
        let leaf_hashes: Vec<Hash> = vec![];
        let (node_hashes, root_hash, height) = build_internal_nodes(&leaf_hashes, 16);

        assert!(node_hashes.is_empty());
        assert_eq!(root_hash, [0u8; 32].into());
        assert_eq!(height, 1);
    }

    #[test]
    fn test_build_chunks_respects_size_limit() {
        let entries: Vec<([u8; 32], Vec<u8>)> = (0..100)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i;
                (key, vec![i; 1000]) // 1KB values
            })
            .collect();

        let params = TreeParams {
            leaf_target_bytes: 10_000, // 10KB chunks
            ..Default::default()
        };

        let chunks = build_chunks(&entries, &params).unwrap();

        // Each chunk should have roughly 10 entries (10KB / 1KB)
        // With borsh overhead, might be slightly fewer
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.uncompressed_len <= params.leaf_target_bytes + 2000);
        }
    }
}
