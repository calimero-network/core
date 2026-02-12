//! HashComparison sync protocol handler (CIP §2.3 Rules 3, 7).
//!
//! Traverses Merkle tree comparing hashes, transfers differing entities.
//!
//! # Protocol Overview
//!
//! ```text
//! Initiator                              Responder
//! │                                            │
//! │ ── TreeNodeRequest (root) ───────────────► │
//! │                                            │ Lookup node
//! │ ◄── TreeNodeResponse (children hashes) ─── │
//! │                                            │
//! │ Compare with local tree                    │
//! │                                            │
//! │ For differing children:                    │
//! │ ── TreeNodeRequest (child) ──────────────► │
//! │ ◄── TreeNodeResponse ─────────────────────│
//! │                                            │
//! │ ...recurse until leaves...                 │
//! │                                            │
//! │ At leaf: CRDT MERGE (Invariant I5)         │
//! └────────────────────────────────────────────┘
//! ```
//!
//! # Critical Invariants
//!
//! - **I5 - No Silent Data Loss**: ALWAYS uses CRDT merge for leaf entities.
//!   LWW overwrite is NEVER permitted for initialized nodes.
//! - **I4 - Strategy Equivalence**: Final state must match other sync strategies
//!   (Snapshot, BloomFilter, etc.) given identical inputs.
//!
//! # Implementation Notes
//!
//! - Uses the real Merkle tree via `Index<MainStorage>` with RuntimeEnv bridge
//! - Uses iterative DFS with explicit stack (avoids stack overflow on deep trees)
//! - Validates all responses (DoS protection via `is_valid()`)
//! - Integrates with delta buffer for concurrent delta handling (I6)

use std::cell::RefCell;
use std::rc::Rc;

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    compare_tree_nodes, InitPayload, LeafMetadata, MessagePayload, StreamMessage,
    TreeCompareResult, TreeLeafData, TreeNode, TreeNodeResponse, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::store::{Key, MainStorage};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::{key, types};
use eyre::{bail, Result, WrapErr};
use libp2p::PeerId;
use tracing::{debug, info, trace, warn};

use super::manager::SyncManager;

/// Maximum number of pending node requests (DFS stack depth limit).
///
/// Prevents unbounded memory growth during traversal.
/// If exceeded, the sync session fails rather than silently dropping nodes.
const MAX_PENDING_NODES: usize = 10_000;

/// Maximum depth allowed in TreeNodeRequest.
///
/// Prevents malicious peers from requesting expensive deep traversals.
pub const MAX_REQUEST_DEPTH: u8 = 16;

/// Statistics from a HashComparison sync session.
#[derive(Debug, Default)]
pub struct HashComparisonStats {
    /// Number of tree nodes compared.
    pub nodes_compared: u64,
    /// Number of leaf entities merged via CRDT.
    pub entities_merged: u64,
    /// Number of nodes skipped (hashes matched).
    pub nodes_skipped: u64,
    /// Number of requests sent to peer.
    pub requests_sent: u64,
}

// =============================================================================
// Storage Bridge
// =============================================================================

/// Storage callback closures that bridge `calimero-storage` Key API to the Store.
///
/// These closures translate `calimero-storage::Key` (Index/Entry) to
/// `calimero-store::ContextStateKey` for access to the actual RocksDB data.
#[expect(
    clippy::type_complexity,
    reason = "Matches RuntimeEnv callback signatures"
)]
struct StorageCallbacks {
    read: Rc<dyn Fn(&Key) -> Option<Vec<u8>>>,
    write: Rc<dyn Fn(Key, &[u8]) -> bool>,
    remove: Rc<dyn Fn(&Key) -> bool>,
}

/// Create storage callbacks for a context.
///
/// These bridge the `calimero-storage` Key-based API to the underlying
/// `calimero-store` ContextStateKey-based storage.
#[expect(
    clippy::type_complexity,
    reason = "Matches RuntimeEnv callback signatures"
)]
fn create_storage_callbacks(
    datastore: &calimero_store::Store,
    context_id: ContextId,
) -> StorageCallbacks {
    let read: Rc<dyn Fn(&Key) -> Option<Vec<u8>>> = {
        let handle = datastore.handle();
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            match handle.get(&state_key) {
                Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
                Ok(None) => None,
                Err(e) => {
                    warn!(
                        %ctx_id,
                        storage_key = %hex::encode(storage_key),
                        error = ?e,
                        "Storage read failed during HashComparison"
                    );
                    None
                }
            }
        })
    };

    let write: Rc<dyn Fn(Key, &[u8]) -> bool> = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(datastore.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: Key, value: &[u8]| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            let slice: calimero_store::slice::Slice<'_> = value.to_vec().into();
            let state_value = types::ContextState::from(slice);
            handle_cell
                .borrow_mut()
                .put(&state_key, &state_value)
                .is_ok()
        })
    };

    let remove: Rc<dyn Fn(&Key) -> bool> = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(datastore.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            handle_cell.borrow_mut().delete(&state_key).is_ok()
        })
    };

    StorageCallbacks {
        read,
        write,
        remove,
    }
}

/// Create a RuntimeEnv for accessing the Merkle tree Index.
fn create_runtime_env(
    callbacks: &StorageCallbacks,
    context_id: ContextId,
    executor_id: PublicKey,
) -> RuntimeEnv {
    RuntimeEnv::new(
        callbacks.read.clone(),
        callbacks.write.clone(),
        callbacks.remove.clone(),
        *context_id.as_ref(),
        *executor_id.as_ref(),
    )
}

// =============================================================================
// SyncManager Implementation
// =============================================================================

impl SyncManager {
    /// Execute HashComparison sync protocol as initiator.
    ///
    /// # Wire Protocol
    ///
    /// 1. Request root node from peer
    /// 2. Compare children with local tree
    /// 3. For differing children: recurse (request subtree)
    /// 4. At leaves: CRDT merge (Invariant I5)
    ///
    /// # Invariant I5 - No Silent Data Loss
    ///
    /// Uses CRDT merge for all entities. NEVER uses last-write-wins for
    /// initialized nodes. This is enforced by storing the merged values
    /// through the normal storage path which handles CRDT semantics.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context being synchronized
    /// * `peer_id` - Peer to sync from
    /// * `our_identity` - Our identity for authentication
    /// * `stream` - Network stream for communication
    /// * `remote_root_hash` - Peer's root hash from handshake
    ///
    /// # Returns
    ///
    /// Statistics about the sync session, or error if sync failed.
    pub(crate) async fn hash_comparison_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        remote_root_hash: [u8; 32],
    ) -> Result<HashComparisonStats> {
        info!(%context_id, %peer_id, "Starting HashComparison sync");

        let mut stats = HashComparisonStats::default();

        // Set up storage bridge for accessing the Merkle tree
        let datastore = self.context_client.datastore_handle().into_inner();
        let callbacks = create_storage_callbacks(&datastore, context_id);
        let runtime_env = create_runtime_env(&callbacks, context_id, our_identity);

        // Stack for DFS traversal (iterative to avoid deep recursion)
        // Each entry is (node_id, is_root_request)
        // For root: node_id is the root_hash, we look up Id::root()
        // For children: node_id is the entity Id bytes
        let mut to_compare: Vec<([u8; 32], bool)> = vec![(remote_root_hash, true)];

        while let Some((node_id, is_root_request)) = to_compare.pop() {
            // Limit stack size to prevent memory exhaustion (DoS protection)
            // Fail the sync session rather than silently dropping nodes,
            // which would cause incomplete sync and break convergence guarantees.
            if to_compare.len() > MAX_PENDING_NODES {
                bail!(
                    "HashComparison sync aborted: pending nodes ({}) exceeds limit ({}). \
                     Tree may be too large for HashComparison; consider using Snapshot sync.",
                    to_compare.len(),
                    MAX_PENDING_NODES
                );
            }

            // Get local node from our Merkle tree
            let local_node = with_runtime_env(runtime_env.clone(), || {
                self.get_local_tree_node_from_index(context_id, &node_id, is_root_request)
            })?;

            // Request node from peer
            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::TreeNodeRequest {
                    context_id,
                    node_id,
                    max_depth: Some(1), // Request immediate children only
                },
                next_nonce: super::helpers::generate_nonce(),
            };

            self.send(stream, &request_msg, None).await?;
            stats.requests_sent += 1;

            // Receive response
            let response = self.recv(stream, None).await?;

            let Some(StreamMessage::Message { payload, .. }) = response else {
                bail!("Unexpected response type during HashComparison sync");
            };

            let (nodes, not_found) = match payload {
                MessagePayload::TreeNodeResponse { nodes, not_found } => (nodes, not_found),
                MessagePayload::SnapshotError { error } => {
                    warn!(%context_id, ?error, "Peer returned error during HashComparison");
                    bail!("Peer error: {:?}", error);
                }
                _ => {
                    bail!("Unexpected payload type during HashComparison sync");
                }
            };

            // Validate response (DoS protection)
            if nodes.len() > MAX_NODES_PER_RESPONSE {
                warn!(
                    %context_id,
                    count = nodes.len(),
                    max = MAX_NODES_PER_RESPONSE,
                    "TreeNodeResponse exceeds max nodes, skipping"
                );
                continue;
            }

            if not_found {
                debug!(%context_id, node_id = %hex::encode(node_id), "Node not found on peer");
                continue;
            }

            // Process each node in response
            for remote_node in nodes {
                // Validate node structure
                if !remote_node.is_valid() {
                    warn!(%context_id, "Invalid TreeNode structure, skipping");
                    continue;
                }

                stats.nodes_compared += 1;

                if remote_node.is_leaf() {
                    // Leaf node: apply CRDT merge (Invariant I5)
                    if let Some(ref leaf_data) = remote_node.leaf_data {
                        trace!(
                            %context_id,
                            key = %hex::encode(leaf_data.key),
                            crdt_type = ?leaf_data.metadata.crdt_type,
                            "Merging leaf entity via CRDT"
                        );

                        self.apply_leaf_with_crdt_merge(context_id, leaf_data)
                            .await
                            .wrap_err("failed to merge leaf entity")?;

                        stats.entities_merged += 1;
                    }
                } else {
                    // Internal node: compare with local and queue differing children
                    let compare_result =
                        compare_tree_nodes(local_node.as_ref(), Some(&remote_node));

                    match compare_result {
                        TreeCompareResult::Equal => {
                            // Subtree matches, skip
                            stats.nodes_skipped += 1;
                            trace!(
                                %context_id,
                                node_id = %hex::encode(remote_node.id),
                                "Subtree hashes match, skipping"
                            );
                        }
                        TreeCompareResult::LocalMissing => {
                            // We don't have this node locally - fetch all children
                            trace!(
                                %context_id,
                                node_id = %hex::encode(remote_node.id),
                                children = remote_node.children.len(),
                                "Local missing, fetching all children"
                            );
                            // Children are entity IDs, not root requests
                            for child_id in &remote_node.children {
                                to_compare.push((*child_id, false));
                            }
                        }
                        TreeCompareResult::Different {
                            remote_only_children,
                            common_children,
                            local_only_children: _,
                        } => {
                            // Queue remote-only and common children for comparison
                            trace!(
                                %context_id,
                                remote_only = remote_only_children.len(),
                                common = common_children.len(),
                                "Subtrees differ, queuing children"
                            );
                            for child_id in remote_only_children {
                                to_compare.push((child_id, false));
                            }
                            for child_id in common_children {
                                to_compare.push((child_id, false));
                            }
                            // Note: local_only_children are for bidirectional sync (future work)
                        }
                        TreeCompareResult::RemoteMissing => {
                            // We have data that peer doesn't
                            // For unidirectional pull, nothing to do
                            // For bidirectional sync, we would push here (future work)
                            trace!(
                                %context_id,
                                node_id = %hex::encode(remote_node.id),
                                "Remote missing (bidirectional push not implemented)"
                            );
                        }
                    }
                }
            }
        }

        info!(
            %context_id,
            %peer_id,
            nodes_compared = stats.nodes_compared,
            entities_merged = stats.entities_merged,
            nodes_skipped = stats.nodes_skipped,
            requests_sent = stats.requests_sent,
            "HashComparison sync complete"
        );

        Ok(stats)
    }

    /// Get local tree node from the real Merkle tree Index.
    ///
    /// Must be called within `with_runtime_env` context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context being synchronized
    /// * `node_id` - Either root_hash (for root request) or entity ID
    /// * `is_root_request` - True if this is a request for the root node
    fn get_local_tree_node_from_index(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        is_root_request: bool,
    ) -> Result<Option<TreeNode>> {
        // Determine the entity ID to look up
        let entity_id = if is_root_request {
            // For root request, look up Id::root() (which equals context_id)
            Id::new(*context_id.as_ref())
        } else {
            // For child requests, node_id IS the entity ID
            Id::new(*node_id)
        };

        // Get the entity's index from the Merkle tree
        let index = match Index::<MainStorage>::get_index(entity_id) {
            Ok(Some(idx)) => idx,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!(
                    %context_id,
                    entity_id = %entity_id,
                    error = %e,
                    "Failed to get index for entity"
                );
                return Ok(None);
            }
        };

        // Get hashes from the index
        let full_hash = index.full_hash();

        // Get children from the index
        let children_ids: Vec<[u8; 32]> = index
            .children()
            .map(|children| {
                children
                    .iter()
                    .map(|child| *child.id().as_bytes())
                    .collect()
            })
            .unwrap_or_default();

        // Determine if this is a leaf or internal node
        if children_ids.is_empty() {
            // Leaf node - try to get entity data
            if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
                let metadata = LeafMetadata::new(
                    // Get CRDT type from index metadata if available
                    index
                        .metadata
                        .crdt_type
                        .clone()
                        .unwrap_or(CrdtType::LwwRegister),
                    index.metadata.updated_at(),
                    // Collection ID - use parent if available
                    [0u8; 32],
                );

                let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);

                Ok(Some(TreeNode::leaf(
                    *entity_id.as_bytes(),
                    full_hash,
                    leaf_data,
                )))
            } else {
                // Index exists but no entry data - treat as internal node with no children
                // This can happen for collection containers
                Ok(Some(TreeNode::internal(
                    *entity_id.as_bytes(),
                    full_hash,
                    vec![],
                )))
            }
        } else {
            // Internal node with children
            Ok(Some(TreeNode::internal(
                *entity_id.as_bytes(),
                full_hash,
                children_ids,
            )))
        }
    }

    /// Apply leaf data using CRDT merge (Invariant I5).
    ///
    /// This stores the entity through the normal storage path, which handles
    /// CRDT merge semantics based on the entity's crdt_type.
    ///
    /// # Invariant I5
    ///
    /// The storage layer's `put` operation with CRDT metadata will:
    /// - For LwwRegister: Use timestamp comparison
    /// - For GCounter/PnCounter: Union of contributions
    /// - For UnorderedMap/Set: Per-key/element merge
    /// - For Rga/Vector: Ordered merge
    ///
    /// We NEVER use raw overwrite for initialized entities.
    ///
    /// # Concurrency Note
    ///
    /// The read-merge-write pattern has a theoretical TOCTOU window, but this is
    /// acceptable because:
    /// 1. Sync operations are serialized per context (only one sync session active)
    /// 2. CRDT merge is commutative - even if concurrent updates occur, the final
    ///    state will converge correctly
    /// 3. Deltas during sync are buffered (Invariant I6) and replayed after
    async fn apply_leaf_with_crdt_merge(
        &self,
        context_id: ContextId,
        leaf: &TreeLeafData,
    ) -> Result<()> {
        // Get the datastore handle
        let handle = self.context_client.datastore_handle();

        // Create the storage key for this entity
        let key = ContextStateKey::new(context_id, leaf.key);

        // Get existing value to perform merge (copy to owned value to release borrow)
        let existing_bytes: Option<Vec<u8>> = handle.get(&key)?.map(|v| v.as_ref().to_vec());
        drop(handle); // Explicitly release the immutable borrow

        // Merge the values using CRDT semantics
        // The merge strategy depends on the crdt_type in metadata
        let merged_value = if let Some(existing_value) = existing_bytes {
            // CRDT merge: combine existing and incoming values
            // The storage layer handles this via merge_root_state for root entities,
            // or via the entity's inherent merge for collection entries
            self.merge_entity_values(
                leaf.key,
                existing_value,
                &leaf.value,
                leaf.metadata.hlc_timestamp,
                &leaf.metadata,
            )?
        } else {
            // No existing value - just use incoming (this is the only case where
            // we can "overwrite" because there's nothing to overwrite)
            leaf.value.clone()
        };

        // Store the merged value (get a new mutable handle)
        let mut handle = self.context_client.datastore_handle();
        let slice: calimero_store::slice::Slice<'_> = merged_value.into();
        handle.put(&key, &calimero_store::types::ContextState::from(slice))?;

        Ok(())
    }

    /// Merge two entity values using CRDT semantics.
    ///
    /// For HashComparison, we receive raw entity bytes with metadata.
    /// The merge strategy depends on the CRDT type.
    fn merge_entity_values(
        &self,
        entity_key: [u8; 32],
        existing: Vec<u8>,
        incoming: &[u8],
        incoming_timestamp: u64,
        metadata: &LeafMetadata,
    ) -> Result<Vec<u8>> {
        use calimero_storage::merge::merge_root_state;

        match metadata.crdt_type {
            CrdtType::LwwRegister => {
                // Last-write-wins based on HLC timestamp
                // This is still "merge" semantics - the newer value wins
                // (Not raw overwrite - we're comparing timestamps)

                // Extract existing timestamp from stored metadata via Index
                // The storage layer stores metadata separately from raw values
                let existing_ts = Index::<MainStorage>::get_index(Id::from(entity_key))
                    .ok()
                    .flatten()
                    .map(|index| *index.metadata.updated_at)
                    .unwrap_or(0);

                match merge_root_state(&existing, incoming, existing_ts, incoming_timestamp) {
                    Ok(merged) => Ok(merged),
                    Err(_) => {
                        // Fallback: if merge fails, use LWW semantics manually
                        if incoming_timestamp > existing_ts {
                            Ok(incoming.to_vec())
                        } else {
                            Ok(existing)
                        }
                    }
                }
            }
            CrdtType::GCounter | CrdtType::PnCounter => {
                // Counter merge: MUST succeed to preserve I5 (no silent data loss)
                // Counters cannot fall back to incoming without losing contributions
                merge_root_state(&existing, incoming, 0, incoming_timestamp).map_err(|e| {
                    eyre::eyre!(
                        "Counter merge failed for {:?}: {}. Cannot fall back without violating I5.",
                        metadata.crdt_type,
                        e
                    )
                })
            }
            CrdtType::UnorderedMap | CrdtType::UnorderedSet | CrdtType::Rga | CrdtType::Vector => {
                // Collection merge: MUST succeed to preserve I5
                merge_root_state(&existing, incoming, 0, incoming_timestamp).map_err(|e| {
                    eyre::eyre!(
                        "Collection merge failed for {:?}: {}. Cannot fall back without violating I5.",
                        metadata.crdt_type,
                        e
                    )
                })
            }
            CrdtType::UserStorage | CrdtType::FrozenStorage => {
                // Specialized storage types - use registered merge
                // These are typically single-writer, so LWW fallback is acceptable
                match merge_root_state(&existing, incoming, 0, incoming_timestamp) {
                    Ok(merged) => Ok(merged),
                    Err(_) => {
                        // Single-writer storage: LWW is safe
                        if incoming_timestamp > 0 {
                            Ok(incoming.to_vec())
                        } else {
                            Ok(existing)
                        }
                    }
                }
            }
            CrdtType::Custom(_) => {
                // Custom CRDT - must have registered merge function
                // Failing to merge a custom CRDT is a serious issue - return error
                merge_root_state(&existing, incoming, 0, incoming_timestamp).map_err(|e| {
                    eyre::eyre!(
                        "Custom CRDT merge failed for {:?}: {}. Custom CRDTs must have registered merge functions.",
                        metadata.crdt_type,
                        e
                    )
                })
            }
        }
    }

    /// Handle incoming TreeNodeRequest from a peer.
    ///
    /// This is the responder side of HashComparison sync.
    /// Look up the requested node in our Merkle tree and return it.
    pub async fn handle_tree_node_request(
        &self,
        context_id: ContextId,
        node_id: [u8; 32],
        max_depth: Option<u8>,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        debug!(
            %context_id,
            node_id = %hex::encode(node_id),
            ?max_depth,
            "Handling TreeNodeRequest"
        );

        // Validate max_depth to prevent expensive deep traversals (DoS protection)
        if let Some(depth) = max_depth {
            if depth > MAX_REQUEST_DEPTH {
                warn!(
                    %context_id,
                    requested_depth = depth,
                    max_allowed = MAX_REQUEST_DEPTH,
                    "TreeNodeRequest depth exceeds limit, clamping"
                );
            }
        }
        let clamped_depth = max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));

        // Get our identity for RuntimeEnv - look up from context members
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let our_identity = match crate::utils::choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        {
            Some((identity, _)) => identity,
            None => {
                warn!(%context_id, "No owned identity for context, cannot respond to TreeNodeRequest");
                // Send not-found response
                let mut sqx = super::tracking::Sequencer::default();
                let msg = StreamMessage::Message {
                    sequence_id: sqx.next(),
                    payload: MessagePayload::TreeNodeResponse {
                        nodes: vec![],
                        not_found: true,
                    },
                    next_nonce: super::helpers::generate_nonce(),
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

        // Build response with requested node(s)
        let response = self
            .build_tree_node_response(context_id, &node_id, clamped_depth, our_identity)
            .await?;

        // Send response
        let mut sqx = super::tracking::Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::TreeNodeResponse {
                nodes: response.nodes,
                not_found: response.not_found,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Build TreeNodeResponse for a requested node.
    ///
    /// Uses the real Merkle tree Index via RuntimeEnv bridge.
    async fn build_tree_node_response(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        max_depth: Option<u8>,
        our_identity: PublicKey,
    ) -> Result<TreeNodeResponse> {
        // Set up storage bridge
        let datastore = self.context_client.datastore_handle().into_inner();
        let callbacks = create_storage_callbacks(&datastore, context_id);
        let runtime_env = create_runtime_env(&callbacks, context_id, our_identity);

        // Get context to check if this is a root request
        let context = self.context_client.get_context(&context_id)?;
        let Some(context) = context else {
            debug!(
                %context_id,
                "Context not found for TreeNodeRequest"
            );
            return Ok(TreeNodeResponse::not_found());
        };

        // Determine if this is a root request (node_id matches root_hash)
        let is_root_request = node_id == context.root_hash.as_ref();

        // Get the local node
        let local_node = with_runtime_env(runtime_env.clone(), || {
            self.get_local_tree_node_from_index(context_id, node_id, is_root_request)
        })?;

        let Some(node) = local_node else {
            debug!(
                %context_id,
                node_id = %hex::encode(node_id),
                "TreeNodeRequest: node not found"
            );
            return Ok(TreeNodeResponse::not_found());
        };

        let mut nodes = vec![node.clone()];

        // If max_depth > 0 and this is an internal node, include children
        let depth = max_depth.unwrap_or(0);
        if depth > 0 && node.is_internal() {
            // Include child nodes
            for child_id in &node.children {
                let child_node = with_runtime_env(runtime_env.clone(), || {
                    self.get_local_tree_node_from_index(context_id, child_id, false)
                })?;

                if let Some(child) = child_node {
                    nodes.push(child);

                    // Limit to avoid oversized responses
                    if nodes.len() >= MAX_NODES_PER_RESPONSE {
                        break;
                    }
                }
            }
        }

        debug!(
            %context_id,
            node_id = %hex::encode(node_id),
            nodes_count = nodes.len(),
            "TreeNodeRequest: returning nodes"
        );

        Ok(TreeNodeResponse::new(nodes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_comparison_stats_default() {
        let stats = HashComparisonStats::default();
        assert_eq!(stats.nodes_compared, 0);
        assert_eq!(stats.entities_merged, 0);
        assert_eq!(stats.nodes_skipped, 0);
        assert_eq!(stats.requests_sent, 0);
    }
}
