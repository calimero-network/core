//! SyncManager Merkle request/response logic.

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    CompressedChunk, MerkleCursor, MerkleErrorCode, MerkleSyncFrame, MessagePayload, NodeDigest,
    NodeId, StreamMessage, TreeParams,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::{Generic as GenericKey, SCOPE_SIZE};
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use eyre::Result;
use tracing::{debug, info, warn};

use super::traversal::{MerkleTraversalState, TraversalAction};
use super::tree::{is_empty_tree_hash, MerkleTree};
use super::validation::{
    validate_merkle_sync_request, MerkleSyncBoundary, MerkleSyncRequestValidation, MerkleSyncResult,
};
use crate::sync::manager::SyncManager;
use crate::sync::tracking::Sequencer;

/// Scope for Merkle cursor persistence in the Generic column.
/// Exactly 16 bytes to match SCOPE_SIZE.
const MERKLE_CURSOR_SCOPE: [u8; SCOPE_SIZE] = *b"merkle-cursor\0\0\0";

/// How often to checkpoint the cursor during traversal (in chunks applied).
const CURSOR_CHECKPOINT_INTERVAL: usize = 10;

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
        cache_key: crate::sync::manager::MerkleTreeCacheKey,
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
                crate::sync::manager::CachedMerkleTree {
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
            let response =
                crate::sync::stream::recv(stream, None, self.sync_config.timeout).await?;

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
                            next_nonce: crate::sync::helpers::generate_nonce(),
                        };
                        crate::sync::stream::send(stream, &reply, None).await?;
                    }
                    MerkleSyncFrame::LeafRequest { leaves } => {
                        let chunks =
                            self.handle_leaf_request(tree, &leaves, page_limit, byte_limit);
                        let reply = StreamMessage::Message {
                            sequence_id: sqx.next(),
                            payload: MessagePayload::MerkleSyncFrame {
                                frame: MerkleSyncFrame::LeafReply { leaves: chunks },
                            },
                            next_nonce: crate::sync::helpers::generate_nonce(),
                        };
                        crate::sync::stream::send(stream, &reply, None).await?;
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
            next_nonce: crate::sync::helpers::generate_nonce(),
        };
        crate::sync::stream::send(stream, &msg, None).await
    }

    // =========================================================================
    // Merkle Sync Requester Path
    // =========================================================================

    /// Request and apply Merkle sync from a peer, optionally resuming from a cursor.
    ///
    /// This is called when:
    /// 1. Delta sync returned `SnapshotRequired` (pruned history)
    /// 2. We have local state (not uninitialized)
    /// 3. Peer supports Merkle sync (`tree_params` present in boundary response)
    ///
    /// If `resume_cursor` is `Some`, the sync resumes from the given traversal state.
    /// This is useful for resuming interrupted syncs without starting over.
    ///
    /// ## Resumable Sync
    ///
    /// The cursor is periodically checkpointed to storage during traversal. If the sync
    /// is interrupted (connection drop, timeout, crash), it can be resumed by loading
    /// the persisted cursor and passing it to this method.
    ///
    /// ## Cursor Lifecycle
    ///
    /// 1. If `resume_cursor` is provided, it's persisted before sending the request
    /// 2. During traversal, the cursor is checkpointed every N chunks
    /// 3. On successful completion, the cursor is cleared from storage
    /// 4. On failure, the cursor remains for later resume attempts
    pub(in crate::sync) async fn request_merkle_sync_with_cursor(
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
                    eyre::bail!("Resume cursor exceeds 64 KiB limit - use snapshot sync instead");
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

        // Persist initial cursor before starting (for crash recovery)
        if let Some(ref cursor) = resume_cursor {
            self.set_merkle_cursor(context_id, cursor, &boundary.boundary_root_hash)?;
        }

        // Send MerkleSyncRequest
        let init_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: calimero_node_primitives::sync::InitPayload::MerkleSyncRequest {
                context_id,
                boundary_root_hash: boundary.boundary_root_hash,
                tree_params: boundary.tree_params.clone(),
                page_limit: crate::sync::snapshot::DEFAULT_PAGE_LIMIT,
                byte_limit: crate::sync::snapshot::DEFAULT_PAGE_BYTE_LIMIT,
                resume_cursor: cursor_bytes,
                requester_root_hash: Some(local_tree.root_hash),
            },
            next_nonce: crate::sync::helpers::generate_nonce(),
        };
        crate::sync::stream::send(stream, &init_msg, None).await?;

        // Perform BFS traversal to find and fetch mismatched leaves
        let (mut result, covered_ranges) = self
            .perform_merkle_traversal(
                context_id,
                &boundary.boundary_root_hash,
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
            next_nonce: crate::sync::helpers::generate_nonce(),
        };
        crate::sync::stream::send(stream, &done_msg, None).await?;

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

        // Clear the persisted cursor on successful completion
        self.clear_merkle_cursor(context_id)?;

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
    /// The cursor is periodically checkpointed to storage, allowing the sync to
    /// resume from where it left off if interrupted.
    ///
    /// This is a thin async orchestrator that delegates traversal decisions to
    /// the pure `MerkleTraversalState` state machine.
    async fn perform_merkle_traversal(
        &self,
        context_id: ContextId,
        boundary_root_hash: &Hash,
        stream: &mut Stream,
        local_tree: &MerkleTree,
        tree_params: &TreeParams,
        resume_cursor: Option<MerkleCursor>,
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
                    crate::sync::snapshot::DEFAULT_PAGE_LIMIT as usize,
                )
            }
            None => MerkleTraversalState::new(
                local_tree.root_id(),
                tree_params.clone(),
                crate::sync::snapshot::DEFAULT_PAGE_LIMIT as usize,
            ),
        };

        let mut sqx = Sequencer::default();
        let mut last_checkpoint_chunks = 0;

        // Main traversal loop - orchestrates I/O based on state machine actions
        loop {
            match state.next_action() {
                TraversalAction::RequestNodes(batch) => {
                    let request = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::MerkleSyncFrame {
                            frame: MerkleSyncFrame::NodeRequest { nodes: batch },
                        },
                        next_nonce: crate::sync::helpers::generate_nonce(),
                    };
                    crate::sync::stream::send(stream, &request, None).await?;

                    // Wait for NodeReply
                    let response =
                        crate::sync::stream::recv(stream, None, self.sync_config.timeout).await?;
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
                        next_nonce: crate::sync::helpers::generate_nonce(),
                    };
                    crate::sync::stream::send(stream, &request, None).await?;

                    // Wait for LeafReply
                    let response =
                        crate::sync::stream::recv(stream, None, self.sync_config.timeout).await?;
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

                            // Checkpoint cursor periodically after applying chunks
                            let chunks_since_checkpoint =
                                state.chunks_transferred - last_checkpoint_chunks;
                            if chunks_since_checkpoint >= CURSOR_CHECKPOINT_INTERVAL {
                                if let Some(cursor) = state.to_cursor() {
                                    if let Err(e) = self.set_merkle_cursor(
                                        context_id,
                                        &cursor,
                                        boundary_root_hash,
                                    ) {
                                        warn!(
                                            %context_id,
                                            error = %e,
                                            "Failed to checkpoint Merkle cursor"
                                        );
                                    }
                                    last_checkpoint_chunks = state.chunks_transferred;
                                }
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
    fn apply_merkle_chunk(&self, context_id: ContextId, chunk: &CompressedChunk) -> Result<usize> {
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

        let records = crate::sync::snapshot::decode_snapshot_records(&decompressed)?;
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
        let sorted_ranges = super::validation::sort_ranges(chunk_ranges);

        let mut handle = self.context_client.datastore_handle();

        let keys_to_delete: Vec<[u8; 32]> = {
            let mut iter = handle.iter::<ContextStateKey>()?;
            let mut keys = Vec::new();
            for (key_result, _) in iter.entries() {
                let key = key_result?;
                if key.context_id() == context_id {
                    let state_key = key.state_key();
                    // Use pure helper for range check
                    if !super::validation::key_in_sorted_ranges(&state_key, &sorted_ranges) {
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

    // =========================================================================
    // Merkle Cursor Persistence
    // =========================================================================

    /// Persist a Merkle cursor for later resumption.
    ///
    /// The cursor is stored in the Generic column keyed by context_id.
    /// This allows resuming an interrupted Merkle sync without starting over.
    pub(in crate::sync) fn set_merkle_cursor(
        &self,
        context_id: ContextId,
        cursor: &MerkleCursor,
        boundary_root_hash: &Hash,
    ) -> Result<()> {
        let key = GenericKey::new(MERKLE_CURSOR_SCOPE, *context_id);

        // Store cursor + boundary together for validation on resume
        let persisted = PersistedMerkleCursor {
            cursor: cursor.clone(),
            boundary_root_hash: *boundary_root_hash,
        };
        let value_bytes = borsh::to_vec(&persisted)?;
        let value: GenericData<'_> = Slice::from(value_bytes).into();

        let mut handle = self.context_client.datastore_handle();
        handle.put(&key, &value)?;

        debug!(
            %context_id,
            pending_nodes = cursor.pending_nodes.len(),
            pending_leaves = cursor.pending_leaves.len(),
            covered_ranges = cursor.covered_ranges.len(),
            "Persisted Merkle cursor"
        );
        Ok(())
    }

    /// Load a persisted Merkle cursor if one exists.
    ///
    /// Returns the cursor and the boundary root hash it was created for.
    /// The boundary must match for the cursor to be valid.
    ///
    /// If the cursor is corrupted or fails to deserialize, it is treated as stale:
    /// the corrupted entry is cleared and `None` is returned. This prevents a
    /// corrupted cursor from permanently blocking sync.
    pub(in crate::sync) fn get_merkle_cursor(
        &self,
        context_id: ContextId,
    ) -> Result<Option<(MerkleCursor, Hash)>> {
        let key = GenericKey::new(MERKLE_CURSOR_SCOPE, *context_id);
        let handle = self.context_client.datastore_handle();

        // Extract bytes before handle goes out of scope to avoid lifetime issues
        let bytes_opt: Option<Vec<u8>> = handle.get(&key)?.map(|v| v.as_ref().to_vec());

        match bytes_opt {
            Some(bytes) => {
                match borsh::from_slice::<PersistedMerkleCursor>(&bytes) {
                    Ok(persisted) => {
                        debug!(
                            %context_id,
                            pending_nodes = persisted.cursor.pending_nodes.len(),
                            pending_leaves = persisted.cursor.pending_leaves.len(),
                            "Loaded persisted Merkle cursor"
                        );
                        Ok(Some((persisted.cursor, persisted.boundary_root_hash)))
                    }
                    Err(e) => {
                        // Corrupted cursor - treat as stale, clear it, and continue without resume
                        warn!(
                            %context_id,
                            error = %e,
                            "Failed to deserialize persisted Merkle cursor, clearing stale entry"
                        );
                        // Best-effort clear - if this fails, we'll try again next time
                        if let Err(clear_err) = self.clear_merkle_cursor(context_id) {
                            warn!(
                                %context_id,
                                error = %clear_err,
                                "Failed to clear corrupted Merkle cursor"
                            );
                        }
                        Ok(None)
                    }
                }
            }
            None => Ok(None),
        }
    }

    /// Clear the persisted Merkle cursor after successful sync completion.
    pub(in crate::sync) fn clear_merkle_cursor(&self, context_id: ContextId) -> Result<()> {
        let key = GenericKey::new(MERKLE_CURSOR_SCOPE, *context_id);
        let mut handle = self.context_client.datastore_handle();
        handle.delete(&key)?;
        debug!(%context_id, "Cleared persisted Merkle cursor");
        Ok(())
    }
}

/// Persisted cursor with boundary hash for validation.
#[derive(Debug, Clone, borsh::BorshSerialize, borsh::BorshDeserialize)]
struct PersistedMerkleCursor {
    cursor: MerkleCursor,
    boundary_root_hash: Hash,
}
