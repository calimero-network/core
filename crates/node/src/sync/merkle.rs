//! Merkle tree construction and synchronization for efficient partial sync.
//!
//! This module implements Phase 2 of the sync protocol: Merkle tree-based
//! differential sync. It allows nodes with partial state overlap to efficiently
//! identify and transfer only the differing chunks.
//!
//! ## Hashing Rules
//!
//! ```text
//! leaf_hash = H("leaf" || index || payload_hash || uncompressed_len || start_key || end_key)
//! node_hash = H("node" || level || child_hashes...)
//! ```

use std::collections::HashMap;

use calimero_node_primitives::sync::{NodeDigest, NodeId, SnapshotChunk, TreeParams};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use eyre::Result;
use sha2::{Digest, Sha256};
use tracing::debug;

use super::snapshot::CanonicalRecord;

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
fn build_chunks(
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
fn compute_leaf_hash(chunk: &SnapshotChunk) -> Hash {
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
fn build_internal_nodes(leaf_hashes: &[Hash], fanout: usize) -> (HashMap<NodeId, Hash>, Hash, u16) {
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
fn compute_internal_node_hash(level: u16, child_hashes: &[Hash]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"node");
    hasher.update(level.to_le_bytes());
    for child_hash in child_hashes {
        hasher.update(child_hash.as_bytes());
    }

    let hash_bytes: [u8; 32] = hasher.finalize().into();
    hash_bytes.into()
}

// =============================================================================
// SyncManager Merkle Handlers
// =============================================================================

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    MerkleErrorCode, MerkleSyncFrame, MessagePayload, StreamMessage,
};
use tracing::{info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

impl SyncManager {
    /// Handle incoming Merkle sync request from a peer.
    #[expect(clippy::too_many_arguments, reason = "protocol handler")]
    pub async fn handle_merkle_sync_request(
        &self,
        context_id: ContextId,
        boundary_root_hash: Hash,
        tree_params: TreeParams,
        page_limit: u16,
        byte_limit: u32,
        resume_cursor: Option<Vec<u8>>,
        _requester_root_hash: Option<Hash>, // Optional optimization: early-exit if roots match
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        // Verify context exists
        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                warn!(%context_id, "Context not found for Merkle sync request");
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::InvalidBoundary,
                        "Context not found",
                    )
                    .await;
            }
        };

        // Verify boundary is still valid
        if context.root_hash != boundary_root_hash {
            warn!(%context_id, "Boundary mismatch for Merkle sync");
            return self
                .send_merkle_error(
                    stream,
                    MerkleErrorCode::InvalidBoundary,
                    "Boundary root hash mismatch",
                )
                .await;
        }

        // Verify tree params are compatible
        let our_params = TreeParams::default();
        if !our_params.is_compatible(&tree_params) {
            warn!(%context_id, "Incompatible tree params for Merkle sync");
            return self
                .send_merkle_error(
                    stream,
                    MerkleErrorCode::IncompatibleParams,
                    "Tree parameters mismatch",
                )
                .await;
        }

        info!(
            %context_id,
            %boundary_root_hash,
            page_limit,
            byte_limit,
            has_cursor = resume_cursor.is_some(),
            "Handling Merkle sync request"
        );

        // Build or retrieve cached Merkle tree
        let handle = self.context_client.datastore_handle();
        let tree = MerkleTree::build(&handle, context_id, &tree_params)?;

        // Process frames until Done or error
        self.process_merkle_frames(stream, &tree, page_limit, byte_limit)
            .await
    }

    /// Process Merkle sync frames from the requester.
    async fn process_merkle_frames(
        &self,
        stream: &mut Stream,
        tree: &MerkleTree,
        page_limit: u16,
        byte_limit: u32,
    ) -> Result<()> {
        let mut sqx = Sequencer::default();

        loop {
            let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;

            let Some(StreamMessage::Message { payload, .. }) = response else {
                eyre::bail!("Unexpected message during Merkle sync");
            };

            match payload {
                MessagePayload::MerkleSyncFrame { frame } => match frame {
                    MerkleSyncFrame::NodeRequest { nodes } => {
                        let digests = self.handle_node_request(tree, &nodes, page_limit);
                        let reply = StreamMessage::Message {
                            sequence_id: sqx.next(),
                            payload: MessagePayload::MerkleSyncFrame {
                                frame: MerkleSyncFrame::NodeReply { nodes: digests },
                            },
                            next_nonce: super::helpers::generate_nonce(),
                        };
                        super::stream::send(stream, &reply, None).await?;
                    }
                    MerkleSyncFrame::LeafRequest { leaves } => {
                        let chunks = self.handle_leaf_request(tree, &leaves, byte_limit);
                        let reply = StreamMessage::Message {
                            sequence_id: sqx.next(),
                            payload: MessagePayload::MerkleSyncFrame {
                                frame: MerkleSyncFrame::LeafReply { leaves: chunks },
                            },
                            next_nonce: super::helpers::generate_nonce(),
                        };
                        super::stream::send(stream, &reply, None).await?;
                    }
                    MerkleSyncFrame::Done => {
                        debug!("Merkle sync completed by requester");
                        return Ok(());
                    }
                    MerkleSyncFrame::Error { code, message } => {
                        warn!(code, %message, "Merkle sync error from requester");
                        return Ok(());
                    }
                    _ => {
                        warn!("Unexpected Merkle frame type from requester");
                        return self
                            .send_merkle_error(
                                stream,
                                MerkleErrorCode::InvalidBoundary,
                                "Unexpected frame type",
                            )
                            .await;
                    }
                },
                _ => {
                    eyre::bail!("Unexpected payload during Merkle sync");
                }
            }
        }
    }

    /// Handle a NodeRequest by returning node digests.
    fn handle_node_request(
        &self,
        tree: &MerkleTree,
        nodes: &[NodeId],
        page_limit: u16,
    ) -> Vec<NodeDigest> {
        nodes
            .iter()
            .take(page_limit as usize)
            .filter_map(|id| tree.get_node_digest(id))
            .collect()
    }

    /// Handle a LeafRequest by returning snapshot chunks.
    fn handle_leaf_request(
        &self,
        tree: &MerkleTree,
        leaves: &[u64],
        byte_limit: u32,
    ) -> Vec<SnapshotChunk> {
        let mut chunks = Vec::new();
        let mut total_bytes = 0u32;

        for &idx in leaves {
            if let Some(chunk) = tree.get_chunk(idx) {
                let chunk_size = chunk.payload.len() as u32;
                if total_bytes + chunk_size > byte_limit && !chunks.is_empty() {
                    break;
                }
                chunks.push(chunk.clone());
                total_bytes += chunk_size;
            }
        }

        chunks
    }

    /// Send a Merkle sync error frame.
    async fn send_merkle_error(
        &self,
        stream: &mut Stream,
        code: MerkleErrorCode,
        message: &str,
    ) -> Result<()> {
        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::MerkleSyncFrame {
                frame: MerkleSyncFrame::Error {
                    code: code.as_u16(),
                    message: message.to_string(),
                },
            },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &msg, None).await
    }

    // =========================================================================
    // Merkle Sync Requester Path
    // =========================================================================

    /// Request and apply Merkle sync from a peer.
    ///
    /// This is called when:
    /// 1. Delta sync returned `SnapshotRequired` (pruned history)
    /// 2. We have local state (not uninitialized)
    /// 3. Peer supports Merkle sync (`tree_params` present in boundary response)
    pub async fn request_merkle_sync(
        &self,
        context_id: ContextId,
        our_identity: calimero_primitives::identity::PublicKey,
        boundary: &MerkleSyncBoundary,
        stream: &mut Stream,
    ) -> Result<MerkleSyncResult> {
        info!(
            %context_id,
            boundary_root_hash = %boundary.boundary_root_hash,
            "Starting Merkle sync"
        );

        // Build local tree
        let handle = self.context_client.datastore_handle();
        let local_tree = MerkleTree::build(&handle, context_id, &boundary.tree_params)?;

        info!(
            %context_id,
            local_root = %local_tree.root_hash,
            remote_root = %boundary.merkle_root_hash,
            local_leaves = local_tree.leaf_count(),
            "Built local Merkle tree"
        );

        // If roots match, no sync needed
        if local_tree.root_hash == boundary.merkle_root_hash {
            info!(%context_id, "Merkle roots match, no sync needed");
            return Ok(MerkleSyncResult {
                chunks_transferred: 0,
                records_applied: 0,
            });
        }

        // Send MerkleSyncRequest
        let init_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: calimero_node_primitives::sync::InitPayload::MerkleSyncRequest {
                context_id,
                boundary_root_hash: boundary.boundary_root_hash,
                tree_params: boundary.tree_params.clone(),
                page_limit: super::snapshot::DEFAULT_PAGE_LIMIT,
                byte_limit: super::snapshot::DEFAULT_PAGE_BYTE_LIMIT,
                resume_cursor: None,
                requester_root_hash: Some(local_tree.root_hash),
            },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &init_msg, None).await?;

        // Perform BFS traversal to find and fetch mismatched leaves
        let result = self
            .perform_merkle_traversal(context_id, stream, &local_tree, &boundary.tree_params)
            .await?;

        // Send Done frame
        let done_msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::MerkleSyncFrame {
                frame: MerkleSyncFrame::Done,
            },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &done_msg, None).await?;

        info!(
            %context_id,
            chunks_transferred = result.chunks_transferred,
            records_applied = result.records_applied,
            "Merkle sync completed"
        );

        Ok(result)
    }

    /// Perform BFS traversal to find mismatched nodes and fetch leaf chunks.
    async fn perform_merkle_traversal(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
        local_tree: &MerkleTree,
        tree_params: &TreeParams,
    ) -> Result<MerkleSyncResult> {
        let mut pending_nodes: Vec<NodeId> = vec![local_tree.root_id()];
        let mut pending_leaves: Vec<u64> = Vec::new();
        let mut chunks_transferred = 0usize;
        let mut records_applied = 0usize;
        let mut sqx = Sequencer::default();

        // BFS: process nodes level by level
        while !pending_nodes.is_empty() || !pending_leaves.is_empty() {
            // First, request node hashes for pending internal nodes
            if !pending_nodes.is_empty() {
                let batch: Vec<NodeId> = pending_nodes
                    .drain(
                        ..pending_nodes
                            .len()
                            .min(super::snapshot::DEFAULT_PAGE_LIMIT as usize),
                    )
                    .collect();

                let request = StreamMessage::Message {
                    sequence_id: sqx.next(),
                    payload: MessagePayload::MerkleSyncFrame {
                        frame: MerkleSyncFrame::NodeRequest {
                            nodes: batch.clone(),
                        },
                    },
                    next_nonce: super::helpers::generate_nonce(),
                };
                super::stream::send(stream, &request, None).await?;

                // Wait for NodeReply
                let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;
                let Some(StreamMessage::Message { payload, .. }) = response else {
                    eyre::bail!("Unexpected response during Merkle node request");
                };

                match payload {
                    MessagePayload::MerkleSyncFrame {
                        frame:
                            MerkleSyncFrame::NodeReply {
                                nodes: remote_digests,
                            },
                    } => {
                        // Compare with local and find mismatches
                        for remote_digest in &remote_digests {
                            let local_hash = local_tree.get_node_hash(&remote_digest.id);

                            match local_hash {
                                Some(lh) if lh == remote_digest.hash => {
                                    // Match - skip this subtree
                                }
                                _ => {
                                    // Mismatch - drill down
                                    if remote_digest.id.level == 0 {
                                        // Leaf node - queue for fetch
                                        pending_leaves.push(remote_digest.id.index);
                                    } else {
                                        // Internal node - queue children
                                        let children = get_children_ids(
                                            &remote_digest.id,
                                            remote_digest.child_count,
                                            tree_params.fanout,
                                        );
                                        pending_nodes.extend(children);
                                    }
                                }
                            }
                        }
                    }
                    MessagePayload::MerkleSyncFrame {
                        frame: MerkleSyncFrame::Error { code, message },
                    } => {
                        eyre::bail!("Merkle sync error (code {}): {}", code, message);
                    }
                    _ => {
                        eyre::bail!("Unexpected payload during Merkle node request");
                    }
                }
            }

            // Then, fetch any pending leaf chunks
            if !pending_leaves.is_empty() && pending_nodes.is_empty() {
                let batch: Vec<u64> = pending_leaves
                    .drain(
                        ..pending_leaves
                            .len()
                            .min(super::snapshot::DEFAULT_PAGE_LIMIT as usize),
                    )
                    .collect();

                let request = StreamMessage::Message {
                    sequence_id: sqx.next(),
                    payload: MessagePayload::MerkleSyncFrame {
                        frame: MerkleSyncFrame::LeafRequest { leaves: batch },
                    },
                    next_nonce: super::helpers::generate_nonce(),
                };
                super::stream::send(stream, &request, None).await?;

                // Wait for LeafReply
                let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;
                let Some(StreamMessage::Message { payload, .. }) = response else {
                    eyre::bail!("Unexpected response during Merkle leaf request");
                };

                match payload {
                    MessagePayload::MerkleSyncFrame {
                        frame: MerkleSyncFrame::LeafReply { leaves: chunks },
                    } => {
                        for chunk in chunks {
                            let applied = self.apply_merkle_chunk(context_id, &chunk)?;
                            records_applied += applied;
                            chunks_transferred += 1;
                        }
                    }
                    MessagePayload::MerkleSyncFrame {
                        frame: MerkleSyncFrame::Error { code, message },
                    } => {
                        eyre::bail!("Merkle sync error (code {}): {}", code, message);
                    }
                    _ => {
                        eyre::bail!("Unexpected payload during Merkle leaf request");
                    }
                }
            }
        }

        Ok(MerkleSyncResult {
            chunks_transferred,
            records_applied,
        })
    }

    /// Apply a Merkle chunk by replacing the key range.
    ///
    /// This deletes all local keys in [start_key, end_key] and writes the chunk records.
    fn apply_merkle_chunk(&self, context_id: ContextId, chunk: &SnapshotChunk) -> Result<usize> {
        use calimero_store::key::ContextState as ContextStateKey;
        use calimero_store::slice::Slice;
        use calimero_store::types::ContextState as ContextStateValue;

        let mut handle = self.context_client.datastore_handle();

        // Parse the key range from the chunk
        let start_key: [u8; 32] = chunk
            .start_key
            .as_slice()
            .try_into()
            .map_err(|_| eyre::eyre!("Invalid start_key length"))?;
        let end_key: [u8; 32] = chunk
            .end_key
            .as_slice()
            .try_into()
            .map_err(|_| eyre::eyre!("Invalid end_key length"))?;

        // Delete existing keys in the range.
        // TODO: This is O(n) over all context entries. Consider adding range iteration
        // to the store layer for better performance with large state.
        let keys_to_delete: Vec<[u8; 32]> = {
            let mut iter = handle.iter::<ContextStateKey>()?;
            let mut keys = Vec::new();
            for (key_result, _) in iter.entries() {
                let key = key_result?;
                if key.context_id() == context_id {
                    let state_key = key.state_key();
                    if state_key >= start_key && state_key <= end_key {
                        keys.push(state_key);
                    }
                }
            }
            keys
        };

        for state_key in &keys_to_delete {
            handle.delete(&ContextStateKey::new(context_id, *state_key))?;
        }

        // Decode and write chunk records
        let records = super::snapshot::decode_snapshot_records(&chunk.payload)?;
        for (state_key, value) in &records {
            let key = ContextStateKey::new(context_id, *state_key);
            let slice: Slice<'_> = value.clone().into();
            handle.put(&key, &ContextStateValue::from(slice))?;
        }

        debug!(
            %context_id,
            chunk_index = chunk.index,
            deleted = keys_to_delete.len(),
            written = records.len(),
            "Applied Merkle chunk"
        );

        Ok(records.len())
    }
}

/// Get children node IDs for an internal node.
fn get_children_ids(parent: &NodeId, child_count: u16, fanout: u16) -> Vec<NodeId> {
    let child_level = parent.level - 1;
    let first_child_idx = parent.index * fanout as u64;

    (0..child_count as u64)
        .map(|i| NodeId {
            level: child_level,
            index: first_child_idx + i,
        })
        .collect()
}

/// Boundary information for Merkle sync.
#[derive(Debug, Clone)]
pub struct MerkleSyncBoundary {
    pub boundary_root_hash: Hash,
    pub tree_params: TreeParams,
    pub merkle_root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
}

/// Result of a Merkle sync operation.
#[derive(Debug)]
pub struct MerkleSyncResult {
    pub chunks_transferred: usize,
    pub records_applied: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(chunk.uncompressed_len <= params.leaf_target_bytes + 2000); // Allow some overhead
        }
    }
}
