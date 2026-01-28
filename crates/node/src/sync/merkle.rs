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

use calimero_node_primitives::sync::{
    CompressedChunk, NodeDigest, NodeId, SnapshotChunk, TreeParams,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use eyre::Result;
use sha2::{Digest, Sha256};
use tracing::debug;

use super::snapshot::CanonicalRecord;

/// Check if a hash represents an empty Merkle tree (all zeros).
fn is_empty_tree_hash(hash: &Hash) -> bool {
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
        // Get context root hash for validation
        let context_root_hash = self
            .context_client
            .get_context(&context_id)?
            .map(|ctx| ctx.root_hash);

        // Use pure validation function
        let validation = validate_merkle_sync_request(
            context_root_hash,
            boundary_root_hash,
            &tree_params,
            resume_cursor.as_deref(),
        );

        match validation {
            MerkleSyncRequestValidation::Valid { cursor } => {
                // Log cursor info if resuming
                if let Some(ref c) = cursor {
                    info!(
                        %context_id,
                        pending_nodes = c.pending_nodes.len(),
                        pending_leaves = c.pending_leaves.len(),
                        "Resuming Merkle sync from cursor"
                    );
                }
                drop(cursor); // cursor not needed further in this handler
            }
            MerkleSyncRequestValidation::ContextNotFound => {
                warn!(%context_id, "Context not found for Merkle sync request");
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::InvalidBoundary,
                        "Context not found",
                    )
                    .await;
            }
            MerkleSyncRequestValidation::BoundaryMismatch => {
                warn!(%context_id, "Boundary mismatch for Merkle sync");
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::InvalidBoundary,
                        "Boundary root hash mismatch",
                    )
                    .await;
            }
            MerkleSyncRequestValidation::IncompatibleParams => {
                warn!(%context_id, "Incompatible tree params for Merkle sync");
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::IncompatibleParams,
                        "Tree parameters mismatch",
                    )
                    .await;
            }
            MerkleSyncRequestValidation::CursorTooLarge { size, max } => {
                warn!(
                    %context_id,
                    cursor_size = size,
                    max_size = max,
                    "Rejecting oversized resume cursor"
                );
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::ResumeCursorInvalid,
                        "Resume cursor exceeds 64 KiB limit",
                    )
                    .await;
            }
            MerkleSyncRequestValidation::CursorMalformed { error } => {
                warn!(
                    %context_id,
                    error = %error,
                    "Failed to deserialize resume cursor"
                );
                return self
                    .send_merkle_error(
                        stream,
                        MerkleErrorCode::ResumeCursorInvalid,
                        "Malformed resume cursor",
                    )
                    .await;
            }
        }

        info!(
            %context_id,
            %boundary_root_hash,
            page_limit,
            byte_limit,
            "Handling Merkle sync request"
        );

        // Build or retrieve cached Merkle tree
        let cache_key = (context_id, boundary_root_hash);
        let tree = self.get_or_build_merkle_tree(cache_key, &tree_params)?;

        // Process frames until Done or error
        self.process_merkle_frames(stream, &tree, page_limit, byte_limit)
            .await
    }

    /// Get a Merkle tree from cache or build and cache it.
    ///
    /// The cache key is (context_id, boundary_root_hash) so trees are invalidated
    /// when the boundary changes. Uses LRU eviction when cache is full.
    fn get_or_build_merkle_tree(
        &self,
        cache_key: super::manager::MerkleTreeCacheKey,
        tree_params: &TreeParams,
    ) -> Result<MerkleTree> {
        use tokio::time::Instant;

        // Try to get from cache first, updating last_access time
        {
            let mut cache = self
                .merkle_tree_cache
                .write()
                .map_err(|e| eyre::eyre!("Merkle tree cache lock poisoned: {}", e))?;
            if let Some(entry) = cache.get_mut(&cache_key) {
                entry.last_access = Instant::now();
                debug!(
                    context_id = %cache_key.0,
                    boundary_root_hash = %cache_key.1,
                    "Using cached Merkle tree"
                );
                return Ok(entry.tree.clone());
            }
        }

        // Build the tree
        let handle = self.context_client.datastore_handle();
        let tree = MerkleTree::build(&handle, cache_key.0, tree_params)?;

        // Insert into cache (write lock)
        {
            let mut cache = self
                .merkle_tree_cache
                .write()
                .map_err(|e| eyre::eyre!("Merkle tree cache lock poisoned: {}", e))?;

            // Limit cache size to prevent unbounded growth - use LRU eviction
            const MAX_CACHE_ENTRIES: usize = 16;
            if cache.len() >= MAX_CACHE_ENTRIES {
                // Find and remove least recently used entry
                if let Some(lru_key) = cache
                    .iter()
                    .min_by_key(|(_, v)| v.last_access)
                    .map(|(k, _)| *k)
                {
                    debug!(
                        context_id = %lru_key.0,
                        boundary_root_hash = %lru_key.1,
                        "Evicting LRU Merkle tree from cache"
                    );
                    cache.remove(&lru_key);
                }
            }

            debug!(
                context_id = %cache_key.0,
                boundary_root_hash = %cache_key.1,
                "Caching newly built Merkle tree"
            );
            cache.insert(
                cache_key,
                super::manager::CachedMerkleTree {
                    tree: tree.clone(),
                    last_access: Instant::now(),
                },
            );
        }

        Ok(tree)
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
                        let chunks = self.handle_leaf_request(tree, &leaves, page_limit, byte_limit);
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

    /// Handle a LeafRequest by returning compressed chunks for wire transmission.
    ///
    /// Payloads are compressed with lz4_flex. Both `page_limit` (max leaves per reply)
    /// and `byte_limit` (max uncompressed bytes) are enforced per the spec.
    fn handle_leaf_request(
        &self,
        tree: &MerkleTree,
        leaves: &[u64],
        page_limit: u16,
        byte_limit: u32,
    ) -> Vec<CompressedChunk> {
        let mut chunks = Vec::new();
        let mut total_uncompressed = 0u32;

        for &idx in leaves.iter().take(page_limit as usize) {
            if let Some(chunk) = tree.get_chunk(idx) {
                // byte_limit is on uncompressed bytes per spec
                if total_uncompressed + chunk.uncompressed_len > byte_limit && !chunks.is_empty() {
                    break;
                }

                // Compress the payload before sending
                let compressed = lz4_flex::compress_prepend_size(&chunk.payload);

                chunks.push(CompressedChunk {
                    index: chunk.index,
                    start_key: chunk.start_key.clone(),
                    end_key: chunk.end_key.clone(),
                    uncompressed_len: chunk.uncompressed_len,
                    compressed_payload: compressed,
                });
                total_uncompressed += chunk.uncompressed_len;
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
    ///
    /// If `resume_cursor` is provided, the sync resumes from the given traversal state
    /// instead of starting fresh from the root.
    pub async fn request_merkle_sync(
        &self,
        context_id: ContextId,
        our_identity: calimero_primitives::identity::PublicKey,
        boundary: &MerkleSyncBoundary,
        stream: &mut Stream,
    ) -> Result<MerkleSyncResult> {
        self.request_merkle_sync_with_cursor(context_id, our_identity, boundary, stream, None)
            .await
    }

    /// Request and apply Merkle sync from a peer, optionally resuming from a cursor.
    ///
    /// If `resume_cursor` is `Some`, the sync resumes from the given traversal state.
    /// This is useful for resuming interrupted syncs without starting over.
    pub async fn request_merkle_sync_with_cursor(
        &self,
        context_id: ContextId,
        our_identity: calimero_primitives::identity::PublicKey,
        boundary: &MerkleSyncBoundary,
        stream: &mut Stream,
        resume_cursor: Option<calimero_node_primitives::sync::MerkleCursor>,
    ) -> Result<MerkleSyncResult> {
        let is_resume = resume_cursor.is_some();
        info!(
            %context_id,
            boundary_root_hash = %boundary.boundary_root_hash,
            is_resume,
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

        // Handle empty remote tree: delete all local state
        if is_empty_tree_hash(&boundary.merkle_root_hash) {
            info!(%context_id, "Remote tree is empty, deleting all local state");
            let deleted = self.delete_all_context_state(context_id)?;
            return Ok(MerkleSyncResult {
                chunks_transferred: 0,
                records_applied: deleted,
            });
        }

        // Handle empty local tree: need to fetch all from remote
        // When local is empty, the traversal approach doesn't work correctly because
        // local_tree.root_id() won't match the remote tree's structure.
        // Fall back to snapshot sync for this case.
        if is_empty_tree_hash(&local_tree.root_hash) {
            info!(
                %context_id,
                "Local tree is empty, falling back to snapshot sync for full state transfer"
            );
            eyre::bail!("Local tree is empty - use snapshot sync instead of Merkle sync");
        }

        // Serialize resume cursor if provided, with size limit validation
        let cursor_bytes = match resume_cursor.as_ref() {
            Some(cursor) => {
                // Check size limit before serializing to avoid wasted work
                if cursor.exceeds_size_limit() {
                    warn!(
                        %context_id,
                        pending_nodes = cursor.pending_nodes.len(),
                        pending_leaves = cursor.pending_leaves.len(),
                        covered_ranges = cursor.covered_ranges.len(),
                        "Resume cursor exceeds 64 KiB limit, falling back to snapshot sync"
                    );
                    eyre::bail!(
                        "Resume cursor exceeds 64 KiB limit - use snapshot sync instead"
                    );
                }
                let bytes = borsh::to_vec(cursor)?;
                // Double-check actual serialized size
                if bytes.len() > calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE {
                    warn!(
                        %context_id,
                        cursor_size = bytes.len(),
                        max_size = calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE,
                        "Serialized resume cursor exceeds 64 KiB limit"
                    );
                    eyre::bail!(
                        "Serialized resume cursor ({} bytes) exceeds 64 KiB limit",
                        bytes.len()
                    );
                }
                Some(bytes)
            }
            None => None,
        };

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
                resume_cursor: cursor_bytes,
                requester_root_hash: Some(local_tree.root_hash),
            },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &init_msg, None).await?;

        // Perform BFS traversal to find and fetch mismatched leaves
        let (mut result, covered_ranges) = self
            .perform_merkle_traversal(
                context_id,
                stream,
                &local_tree,
                &boundary.tree_params,
                resume_cursor,
            )
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

        // Delete any local keys that fall outside the remote tree's key ranges.
        // This handles the case where local state has keys the remote doesn't have.
        let orphaned_deleted = self.delete_orphaned_keys(context_id, &covered_ranges)?;
        result.records_applied += orphaned_deleted;

        // Verify final state matches expected root hash
        let final_tree = MerkleTree::build(&handle, context_id, &boundary.tree_params)?;
        if final_tree.root_hash != boundary.merkle_root_hash {
            warn!(
                %context_id,
                expected = %boundary.merkle_root_hash,
                actual = %final_tree.root_hash,
                "Post-sync Merkle root verification failed"
            );
            eyre::bail!(
                "Merkle sync verification failed: expected root {}, got {}",
                boundary.merkle_root_hash,
                final_tree.root_hash
            );
        }

        info!(
            %context_id,
            chunks_transferred = result.chunks_transferred,
            records_applied = result.records_applied,
            orphaned_deleted,
            verified_root = %final_tree.root_hash,
            "Merkle sync completed and verified"
        );

        Ok(result)
    }

    /// Perform BFS traversal to find mismatched nodes and fetch leaf chunks.
    ///
    /// Returns the sync result and all key ranges covered by the remote tree.
    /// The key ranges include both fetched chunks and matching local chunks.
    ///
    /// If `resume_cursor` is provided, the traversal starts from that state instead
    /// of the tree root.
    ///
    /// This is a thin async orchestrator that delegates traversal decisions to
    /// the pure `MerkleTraversalState` state machine.
    async fn perform_merkle_traversal(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
        local_tree: &MerkleTree,
        tree_params: &TreeParams,
        resume_cursor: Option<calimero_node_primitives::sync::MerkleCursor>,
    ) -> Result<(MerkleSyncResult, Vec<([u8; 32], [u8; 32])>)> {
        // Initialize state machine from cursor or fresh start
        let mut state = match resume_cursor {
            Some(cursor) => {
                info!(
                    %context_id,
                    pending_nodes = cursor.pending_nodes.len(),
                    pending_leaves = cursor.pending_leaves.len(),
                    covered_ranges = cursor.covered_ranges.len(),
                    "Resuming Merkle traversal from cursor"
                );
                MerkleTraversalState::from_cursor(
                    cursor,
                    tree_params.clone(),
                    super::snapshot::DEFAULT_PAGE_LIMIT as usize,
                )
            }
            None => MerkleTraversalState::new(
                local_tree.root_id(),
                tree_params.clone(),
                super::snapshot::DEFAULT_PAGE_LIMIT as usize,
            ),
        };

        let mut sqx = Sequencer::default();

        // Main traversal loop - orchestrates I/O based on state machine actions
        loop {
            match state.next_action() {
                TraversalAction::RequestNodes(batch) => {
                    let request = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::MerkleSyncFrame {
                            frame: MerkleSyncFrame::NodeRequest { nodes: batch },
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    super::stream::send(stream, &request, None).await?;

                    // Wait for NodeReply
                    let response =
                        super::stream::recv(stream, None, self.sync_config.timeout).await?;
                    let Some(StreamMessage::Message { payload, .. }) = response else {
                        eyre::bail!("Unexpected response during Merkle node request");
                    };

                    match payload {
                        MessagePayload::MerkleSyncFrame {
                            frame: MerkleSyncFrame::NodeReply { nodes: digests },
                        } => {
                            // Delegate comparison logic to pure state machine
                            state.handle_node_reply(local_tree, &digests);
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

                TraversalAction::RequestLeaves(batch) => {
                    let request = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::MerkleSyncFrame {
                            frame: MerkleSyncFrame::LeafRequest { leaves: batch },
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    super::stream::send(stream, &request, None).await?;

                    // Wait for LeafReply
                    let response =
                        super::stream::recv(stream, None, self.sync_config.timeout).await?;
                    let Some(StreamMessage::Message { payload, .. }) = response else {
                        eyre::bail!("Unexpected response during Merkle leaf request");
                    };

                    match payload {
                        MessagePayload::MerkleSyncFrame {
                            frame: MerkleSyncFrame::LeafReply { leaves: chunks },
                        } => {
                            // Delegate chunk processing to pure state machine
                            let reply_result = state.handle_leaf_reply(chunks);

                            // Apply chunks (side effect) - only increment counter on success
                            for chunk in reply_result.chunks_to_apply {
                                let applied = self.apply_merkle_chunk(context_id, &chunk)?;
                                state.record_chunk_applied(applied);
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

                TraversalAction::Done => break,
            }
        }

        Ok((state.result(), state.covered_ranges().to_vec()))
    }

    /// Apply a compressed Merkle chunk by replacing the key range.
    ///
    /// This deletes all local keys in [start_key, end_key] and writes the chunk records.
    /// Merkle sync is reconciliatory, not additive: any local-only data in mismatched
    /// ranges is discarded to match the responder's boundary snapshot.
    fn apply_merkle_chunk(
        &self,
        context_id: ContextId,
        chunk: &CompressedChunk,
    ) -> Result<usize> {
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

        // Decompress the payload (CompressedChunk always has compressed data)
        let decompressed = lz4_flex::decompress_size_prepended(&chunk.compressed_payload)
            .map_err(|e| eyre::eyre!("Failed to decompress chunk payload: {}", e))?;

        if decompressed.len() != chunk.uncompressed_len as usize {
            eyre::bail!(
                "Decompressed size {} doesn't match expected {}",
                decompressed.len(),
                chunk.uncompressed_len
            );
        }

        let records = super::snapshot::decode_snapshot_records(&decompressed)?;
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

    /// Delete all state entries for a context.
    ///
    /// Used when the remote tree is empty or when cleaning up orphaned keys.
    fn delete_all_context_state(&self, context_id: ContextId) -> Result<usize> {
        use calimero_store::key::ContextState as ContextStateKey;

        let mut handle = self.context_client.datastore_handle();

        let keys_to_delete: Vec<[u8; 32]> = {
            let mut iter = handle.iter::<ContextStateKey>()?;
            let mut keys = Vec::new();
            for (key_result, _) in iter.entries() {
                let key = key_result?;
                if key.context_id() == context_id {
                    keys.push(key.state_key());
                }
            }
            keys
        };

        let count = keys_to_delete.len();
        for state_key in keys_to_delete {
            handle.delete(&ContextStateKey::new(context_id, state_key))?;
        }

        info!(%context_id, deleted = count, "Deleted all context state");
        Ok(count)
    }

    /// Delete context state keys that fall outside any of the given chunk ranges.
    ///
    /// This handles the case where local keys exist outside the key ranges
    /// covered by the remote tree's chunks.
    ///
    /// Uses the pure `key_in_sorted_ranges` helper for O(N log M) complexity
    /// where N is number of context keys and M is number of ranges.
    fn delete_orphaned_keys(
        &self,
        context_id: ContextId,
        chunk_ranges: &[([u8; 32], [u8; 32])],
    ) -> Result<usize> {
        use calimero_store::key::ContextState as ContextStateKey;

        if chunk_ranges.is_empty() {
            // No chunks received means remote tree was empty - handled elsewhere
            return Ok(0);
        }

        // Sort ranges using pure helper
        let sorted_ranges = sort_ranges(chunk_ranges);

        let mut handle = self.context_client.datastore_handle();

        let keys_to_delete: Vec<[u8; 32]> = {
            let mut iter = handle.iter::<ContextStateKey>()?;
            let mut keys = Vec::new();
            for (key_result, _) in iter.entries() {
                let key = key_result?;
                if key.context_id() == context_id {
                    let state_key = key.state_key();
                    // Use pure helper for range check
                    if !key_in_sorted_ranges(&state_key, &sorted_ranges) {
                        keys.push(state_key);
                    }
                }
            }
            keys
        };

        let count = keys_to_delete.len();
        for state_key in keys_to_delete {
            handle.delete(&ContextStateKey::new(context_id, state_key))?;
        }

        if count > 0 {
            info!(%context_id, deleted = count, "Deleted orphaned keys outside chunk ranges");
        }
        Ok(count)
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

// =============================================================================
// Pure Traversal State Machine
// =============================================================================

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
        create_resume_cursor(&self.pending_nodes, &self.pending_leaves, &self.covered_ranges)
    }
}

// =============================================================================
// Pure Validation Helpers
// =============================================================================

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

    // =========================================================================
    // Tests for MerkleTraversalState (pure state machine)
    // =========================================================================

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

        // Add some pending leaves
        state.pending_leaves.push(0);
        state.pending_leaves.push(1);

        // Nodes should be processed before leaves
        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => {
                assert_eq!(nodes, vec![root_id]);
            }
            _ => panic!("Expected RequestNodes"),
        }

        // Now pending_nodes is empty, should request leaves
        match state.next_action() {
            TraversalAction::RequestLeaves(leaves) => {
                assert_eq!(leaves, vec![0, 1]);
            }
            _ => panic!("Expected RequestLeaves"),
        }

        // Now both are empty
        assert_eq!(state.next_action(), TraversalAction::Done);
        assert!(state.is_done());
    }

    #[test]
    fn test_traversal_state_batching() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params, 2);

        // Add more nodes than page_limit
        state.pending_nodes = vec![
            NodeId { level: 1, index: 0 },
            NodeId { level: 1, index: 1 },
            NodeId { level: 1, index: 2 },
        ];

        // Should only get page_limit (2) nodes
        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => {
                assert_eq!(nodes.len(), 2);
            }
            _ => panic!("Expected RequestNodes"),
        }

        // Remaining node
        match state.next_action() {
            TraversalAction::RequestNodes(nodes) => {
                assert_eq!(nodes.len(), 1);
            }
            _ => panic!("Expected RequestNodes"),
        }
    }

    #[test]
    fn test_traversal_state_handle_node_reply_match() {
        let params = TreeParams::default();

        // Create a minimal "mock" tree structure for testing
        // We'll create leaf hashes and build internal nodes
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

        let local_tree = MerkleTree {
            params: params.clone(),
            chunks,
            leaf_hashes,
            node_hashes,
            root_hash,
            height,
        };

        // Create state and clear the initial pending node for this test
        let mut state = MerkleTraversalState::new(local_tree.root_id(), params.clone(), 100);
        state.pending_nodes.clear();

        // Simulate receiving a matching node digest
        let remote_digests = vec![NodeDigest {
            id: local_tree.root_id(),
            hash: local_tree.root_hash,
            child_count: 4,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        // Should match and add covered range
        assert_eq!(matches, 1);
        assert_eq!(state.covered_ranges.len(), 1);
        assert!(state.pending_nodes.is_empty()); // No children added
        assert!(state.pending_leaves.is_empty());
    }

    #[test]
    fn test_traversal_state_handle_node_reply_mismatch_internal() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 1, index: 0 }, params.clone(), 100);

        // Create a local tree
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

        let local_tree = MerkleTree {
            params: params.clone(),
            chunks,
            leaf_hashes,
            node_hashes,
            root_hash,
            height,
        };

        // Clear pending_nodes to test mismatch behavior
        state.pending_nodes.clear();

        // Simulate receiving a mismatched internal node digest
        let remote_digests = vec![NodeDigest {
            id: NodeId { level: 1, index: 0 },
            hash: [99u8; 32].into(), // Different hash
            child_count: 4,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        // Should not match, should add children to pending_nodes
        assert_eq!(matches, 0);
        assert!(state.covered_ranges.is_empty());
        assert_eq!(state.pending_nodes.len(), 4); // Children added
    }

    #[test]
    fn test_traversal_state_handle_node_reply_mismatch_leaf() {
        let params = TreeParams::default();
        let mut state = MerkleTraversalState::new(NodeId { level: 0, index: 0 }, params.clone(), 100);
        state.pending_nodes.clear();

        // Create a local tree
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

        let local_tree = MerkleTree {
            params: params.clone(),
            chunks,
            leaf_hashes,
            node_hashes,
            root_hash,
            height,
        };

        // Simulate receiving a mismatched leaf node digest
        let remote_digests = vec![NodeDigest {
            id: NodeId { level: 0, index: 2 },
            hash: [99u8; 32].into(), // Different hash
            child_count: 0,
        }];

        let matches = state.handle_node_reply(&local_tree, &remote_digests);

        // Should not match, should add leaf index to pending_leaves
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
        // chunks_transferred not incremented until record_chunk_applied is called
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

    // =========================================================================
    // Tests for key_in_sorted_ranges (pure helper)
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

        // Key exactly at start
        assert!(key_in_sorted_ranges(&start, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_exact_end() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];

        // Key exactly at end
        assert!(key_in_sorted_ranges(&end, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_middle() {
        let mut start = [0u8; 32];
        start[0] = 10;
        let mut end = [0u8; 32];
        end[0] = 20;

        let ranges = vec![(start, end)];

        // Key in middle
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

        // Key before range
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

        // Key after range
        let mut key = [0u8; 32];
        key[0] = 25;
        assert!(!key_in_sorted_ranges(&key, &ranges));
    }

    #[test]
    fn test_key_in_sorted_ranges_multiple_ranges() {
        let ranges = vec![
            ([0u8; 32], {
                let mut e = [0u8; 32];
                e[0] = 10;
                e
            }),
            ({
                let mut s = [0u8; 32];
                s[0] = 20;
                s
            }, {
                let mut e = [0u8; 32];
                e[0] = 30;
                e
            }),
            ({
                let mut s = [0u8; 32];
                s[0] = 50;
                s
            }, {
                let mut e = [0u8; 32];
                e[0] = 60;
                e
            }),
        ];

        // In first range
        let mut key1 = [0u8; 32];
        key1[0] = 5;
        assert!(key_in_sorted_ranges(&key1, &ranges));

        // In second range
        let mut key2 = [0u8; 32];
        key2[0] = 25;
        assert!(key_in_sorted_ranges(&key2, &ranges));

        // In third range
        let mut key3 = [0u8; 32];
        key3[0] = 55;
        assert!(key_in_sorted_ranges(&key3, &ranges));

        // In gap between ranges
        let mut key4 = [0u8; 32];
        key4[0] = 15;
        assert!(!key_in_sorted_ranges(&key4, &ranges));

        // After all ranges
        let mut key5 = [0u8; 32];
        key5[0] = 70;
        assert!(!key_in_sorted_ranges(&key5, &ranges));
    }

    #[test]
    fn test_sort_ranges() {
        let ranges = vec![
            ({
                let mut s = [0u8; 32];
                s[0] = 30;
                s
            }, {
                let mut e = [0u8; 32];
                e[0] = 40;
                e
            }),
            ({
                let mut s = [0u8; 32];
                s[0] = 10;
                s
            }, {
                let mut e = [0u8; 32];
                e[0] = 20;
                e
            }),
        ];

        let sorted = sort_ranges(&ranges);

        assert_eq!(sorted[0].0[0], 10);
        assert_eq!(sorted[1].0[0], 30);
    }

    // =========================================================================
    // Tests for validate_merkle_sync_request (pure validation)
    // =========================================================================

    #[test]
    fn test_validate_request_valid() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(Some(root_hash), root_hash, &params, None);

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::Valid { cursor: None }
        ));
    }

    #[test]
    fn test_validate_request_context_not_found() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(None, root_hash, &params, None);

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::ContextNotFound
        ));
    }

    #[test]
    fn test_validate_request_boundary_mismatch() {
        let current: Hash = [1u8; 32].into();
        let boundary: Hash = [2u8; 32].into();
        let params = TreeParams::default();

        let result = validate_merkle_sync_request(Some(current), boundary, &params, None);

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::BoundaryMismatch
        ));
    }

    #[test]
    fn test_validate_request_cursor_too_large() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        // Create a cursor that's too large
        let large_cursor = vec![0u8; calimero_node_primitives::sync::MERKLE_CURSOR_MAX_SIZE + 1];

        let result =
            validate_merkle_sync_request(Some(root_hash), root_hash, &params, Some(&large_cursor));

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::CursorTooLarge { .. }
        ));
    }

    #[test]
    fn test_validate_request_cursor_malformed() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        // Create invalid cursor bytes (not valid borsh)
        let malformed_cursor = vec![0xFF, 0xFF, 0xFF, 0xFF];

        let result = validate_merkle_sync_request(
            Some(root_hash),
            root_hash,
            &params,
            Some(&malformed_cursor),
        );

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::CursorMalformed { .. }
        ));
    }

    #[test]
    fn test_validate_request_incompatible_params() {
        let root_hash: Hash = [1u8; 32].into();

        // Create incompatible params (different fanout)
        let incompatible_params = TreeParams {
            fanout: 999, // Very different from default
            ..Default::default()
        };

        let result =
            validate_merkle_sync_request(Some(root_hash), root_hash, &incompatible_params, None);

        assert!(matches!(
            result,
            MerkleSyncRequestValidation::IncompatibleParams
        ));
    }

    #[test]
    fn test_validate_request_returns_parsed_cursor() {
        let root_hash: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        // Create a valid cursor
        let cursor = calimero_node_primitives::sync::MerkleCursor {
            pending_nodes: vec![NodeId { level: 1, index: 0 }],
            pending_leaves: vec![1, 2, 3],
            covered_ranges: vec![],
        };
        let cursor_bytes = borsh::to_vec(&cursor).unwrap();

        let result =
            validate_merkle_sync_request(Some(root_hash), root_hash, &params, Some(&cursor_bytes));

        match result {
            MerkleSyncRequestValidation::Valid {
                cursor: Some(parsed),
            } => {
                assert_eq!(parsed.pending_nodes.len(), 1);
                assert_eq!(parsed.pending_leaves, vec![1, 2, 3]);
            }
            _ => panic!("Expected Valid with parsed cursor"),
        }
    }

    // =========================================================================
    // Tests for parse_boundary_for_merkle (pure parsing)
    // =========================================================================

    #[test]
    fn test_parse_boundary_merkle_supported() {
        let boundary_root: Hash = [1u8; 32].into();
        let merkle_root: Hash = [2u8; 32].into();
        let params = TreeParams::default();
        let dag_heads = vec![[3u8; 32]];

        let result = parse_boundary_for_merkle(
            boundary_root,
            dag_heads.clone(),
            Some(params),
            Some(merkle_root),
        );

        match result {
            BoundaryParseResult::MerkleSupported(boundary) => {
                assert_eq!(boundary.boundary_root_hash, boundary_root);
                assert_eq!(boundary.merkle_root_hash, merkle_root);
                assert_eq!(boundary.dag_heads, dag_heads);
            }
            _ => panic!("Expected MerkleSupported"),
        }
    }

    #[test]
    fn test_parse_boundary_no_tree_params() {
        let boundary_root: Hash = [1u8; 32].into();
        let merkle_root: Hash = [2u8; 32].into();

        let result =
            parse_boundary_for_merkle(boundary_root, vec![], None, Some(merkle_root));

        assert!(matches!(result, BoundaryParseResult::NoTreeParams));
    }

    #[test]
    fn test_parse_boundary_no_merkle_root() {
        let boundary_root: Hash = [1u8; 32].into();
        let params = TreeParams::default();

        let result =
            parse_boundary_for_merkle(boundary_root, vec![], Some(params), None);

        assert!(matches!(result, BoundaryParseResult::NoMerkleRootHash));
    }
}
