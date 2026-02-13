//! Standalone HashComparison protocol implementation.
//!
//! This module contains the protocol logic extracted from `SyncManager` methods
//! into standalone functions that work with any `Store` backend.
//!
//! # Design
//!
//! The protocol is implemented as `HashComparisonProtocol` which implements
//! `SyncProtocolExecutor`. This allows the same code to run in:
//! - Production (with `Store<RocksDB>` and `StreamTransport`)
//! - Simulation (with `Store<InMemoryDB>` and `SimStream`)
//!
//! # Usage
//!
//! ```ignore
//! use calimero_node::sync::hash_comparison_protocol::HashComparisonProtocol;
//! use calimero_node_primitives::sync::SyncProtocolExecutor;
//!
//! // Initiator side
//! let stats = HashComparisonProtocol::run_initiator(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     HashComparisonConfig { remote_root_hash },
//! ).await?;
//!
//! // Responder side (handles one request)
//! HashComparisonProtocol::run_responder(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//! ).await?;
//! ```

use crate::sync::helpers::generate_nonce;
use async_trait::async_trait;
use calimero_node_primitives::sync::{
    compare_tree_nodes, create_runtime_env, InitPayload, LeafMetadata, MessagePayload,
    StreamMessage, SyncProtocolExecutor, SyncTransport, TreeCompareResult, TreeLeafData, TreeNode,
    TreeNodeResponse, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::with_runtime_env;
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::store::MainStorage;
use calimero_store::Store;
use eyre::{bail, Result};
use tracing::{debug, info, trace, warn};

/// Maximum number of pending node requests (DFS stack depth limit).
const MAX_PENDING_NODES: usize = 10_000;

/// Maximum depth allowed in TreeNodeRequest.
pub const MAX_REQUEST_DEPTH: u8 = 16;

/// Configuration for HashComparison initiator.
#[derive(Debug, Clone)]
pub struct HashComparisonConfig {
    /// Remote peer's root hash (from handshake).
    pub remote_root_hash: [u8; 32],
}

/// Statistics from a HashComparison sync session.
#[derive(Debug, Default, Clone)]
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

/// HashComparison sync protocol.
///
/// Implements the Merkle tree traversal protocol (CIP §2.3).
pub struct HashComparisonProtocol;

#[async_trait(?Send)]
impl SyncProtocolExecutor for HashComparisonProtocol {
    type Config = HashComparisonConfig;
    type Stats = HashComparisonStats;

    async fn run_initiator<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
        config: Self::Config,
    ) -> Result<Self::Stats> {
        run_initiator_impl(
            transport,
            store,
            context_id,
            identity,
            config.remote_root_hash,
        )
        .await
    }

    async fn run_responder<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
    ) -> Result<()> {
        run_responder_impl(transport, store, context_id, identity).await
    }
}

// =============================================================================
// Initiator Implementation
// =============================================================================

async fn run_initiator_impl<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    identity: PublicKey,
    remote_root_hash: [u8; 32],
) -> Result<HashComparisonStats> {
    info!(%context_id, "Starting HashComparison sync (initiator)");

    let mut stats = HashComparisonStats::default();

    // Set up storage bridge
    let runtime_env = create_runtime_env(store, context_id, identity);

    // Stack for DFS traversal
    let mut to_compare: Vec<([u8; 32], bool)> = vec![(remote_root_hash, true)];

    while let Some((node_id, is_root_request)) = to_compare.pop() {
        // DoS protection: limit stack size
        if to_compare.len() > MAX_PENDING_NODES {
            bail!(
                "HashComparison sync aborted: pending nodes ({}) exceeds limit ({})",
                to_compare.len(),
                MAX_PENDING_NODES
            );
        }

        // Request node from peer
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            payload: InitPayload::TreeNodeRequest {
                context_id,
                node_id,
                max_depth: Some(1),
            },
            next_nonce: generate_nonce(),
        };

        transport.send(&request_msg).await?;
        stats.requests_sent += 1;

        // Receive response
        let response = transport
            .recv()
            .await?
            .ok_or_else(|| eyre::eyre!("stream closed unexpectedly"))?;

        let StreamMessage::Message { payload, .. } = response else {
            bail!("Unexpected response type during HashComparison sync");
        };

        let (nodes, not_found) = match payload {
            MessagePayload::TreeNodeResponse { nodes, not_found } => (nodes, not_found),
            MessagePayload::SnapshotError { error } => {
                warn!(%context_id, ?error, "Peer returned error");
                bail!("Peer error: {:?}", error);
            }
            _ => bail!("Unexpected payload type"),
        };

        // DoS protection: validate response size
        if nodes.len() > MAX_NODES_PER_RESPONSE {
            warn!(%context_id, count = nodes.len(), "Response too large, skipping");
            continue;
        }

        if not_found {
            debug!(%context_id, node_id = %hex::encode(node_id), "Node not found on peer");
            continue;
        }

        // Process each node
        for remote_node in nodes {
            if !remote_node.is_valid() {
                warn!(%context_id, "Invalid TreeNode, skipping");
                continue;
            }

            stats.nodes_compared += 1;

            if remote_node.is_leaf() {
                // Leaf: apply CRDT merge (Invariant I5)
                if let Some(ref leaf_data) = remote_node.leaf_data {
                    trace!(
                        %context_id,
                        key = %hex::encode(leaf_data.key),
                        "Merging leaf entity"
                    );

                    with_runtime_env(runtime_env.clone(), || {
                        apply_leaf_with_crdt_merge(context_id, leaf_data)
                    })?;
                    stats.entities_merged += 1;
                }
            } else {
                // Internal node: compare with local version
                let is_this_node_root = is_root_request && remote_node.id == node_id;

                let local_version = with_runtime_env(runtime_env.clone(), || {
                    get_local_tree_node(context_id, &remote_node.id, is_this_node_root)
                })?;

                match compare_tree_nodes(local_version.as_ref(), Some(&remote_node)) {
                    TreeCompareResult::Equal => {
                        stats.nodes_skipped += 1;
                        trace!(%context_id, "Subtree matches, skipping");
                    }
                    TreeCompareResult::LocalMissing => {
                        for child_id in &remote_node.children {
                            to_compare.push((*child_id, false));
                        }
                    }
                    TreeCompareResult::Different {
                        remote_only_children,
                        common_children,
                        ..
                    } => {
                        for child_id in remote_only_children {
                            to_compare.push((child_id, false));
                        }
                        for child_id in common_children {
                            to_compare.push((child_id, false));
                        }
                    }
                    TreeCompareResult::RemoteMissing => {
                        // Bidirectional sync: future work
                    }
                }
            }
        }
    }

    // Close the transport to signal completion to the responder
    transport.close().await?;

    info!(
        %context_id,
        nodes_compared = stats.nodes_compared,
        entities_merged = stats.entities_merged,
        nodes_skipped = stats.nodes_skipped,
        "HashComparison sync complete"
    );

    Ok(stats)
}

// =============================================================================
// Responder Implementation
// =============================================================================

async fn run_responder_impl<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    identity: PublicKey,
) -> Result<()> {
    info!(%context_id, "Starting HashComparison sync (responder)");

    // Set up storage bridge (reused across all requests)
    let runtime_env = create_runtime_env(store, context_id, identity);

    // Get our root hash to determine root requests
    let local_root_hash = with_runtime_env(runtime_env.clone(), || {
        Index::<MainStorage>::get_hashes_for(Id::new(*context_id.as_ref()))
            .ok()
            .flatten()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    });

    let mut sequence_id = 0u64;
    let mut requests_handled = 0u64;

    // Handle requests until stream closes
    loop {
        // Receive request (None means stream closed = sync complete)
        let Some(request) = transport.recv().await? else {
            debug!(%context_id, requests_handled, "Stream closed, responder done");
            break;
        };

        let StreamMessage::Init { payload, .. } = request else {
            // Non-Init message might indicate end of sync or protocol error
            debug!(%context_id, "Received non-Init message, ending responder");
            break;
        };

        let InitPayload::TreeNodeRequest {
            node_id, max_depth, ..
        } = payload
        else {
            // Different payload type - might be end of sync
            debug!(%context_id, "Received non-TreeNodeRequest, ending responder");
            break;
        };

        trace!(
            %context_id,
            node_id = %hex::encode(node_id),
            ?max_depth,
            "Handling TreeNodeRequest"
        );

        // Clamp depth for DoS protection
        let clamped_depth = max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));

        let is_root_request = node_id == local_root_hash;

        // Get the requested node
        let local_node = with_runtime_env(runtime_env.clone(), || {
            get_local_tree_node(context_id, &node_id, is_root_request)
        })?;

        let response = if let Some(node) = local_node {
            let mut nodes = vec![node.clone()];

            // Include children if depth > 0
            let depth = clamped_depth.unwrap_or(0);
            if depth > 0 && node.is_internal() {
                for child_id in &node.children {
                    if let Some(child) = with_runtime_env(runtime_env.clone(), || {
                        get_local_tree_node(context_id, child_id, false)
                    })? {
                        nodes.push(child);
                        if nodes.len() >= MAX_NODES_PER_RESPONSE {
                            break;
                        }
                    }
                }
            }

            TreeNodeResponse::new(nodes)
        } else {
            TreeNodeResponse::not_found()
        };

        // Send response
        let msg = StreamMessage::Message {
            sequence_id,
            payload: MessagePayload::TreeNodeResponse {
                nodes: response.nodes,
                not_found: response.not_found,
            },
            next_nonce: generate_nonce(),
        };

        transport.send(&msg).await?;
        sequence_id += 1;
        requests_handled += 1;
    }

    info!(%context_id, requests_handled, "HashComparison responder complete");
    Ok(())
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Get a tree node from the local Merkle tree Index.
fn get_local_tree_node(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root_request: bool,
) -> Result<Option<TreeNode>> {
    let entity_id = if is_root_request {
        Id::new(*context_id.as_ref())
    } else {
        Id::new(*node_id)
    };

    let index = match Index::<MainStorage>::get_index(entity_id) {
        Ok(Some(idx)) => idx,
        Ok(None) => return Ok(None),
        Err(e) => {
            warn!(%context_id, %entity_id, error = %e, "Failed to get index");
            return Ok(None);
        }
    };

    let full_hash = index.full_hash();
    let children_ids: Vec<[u8; 32]> = index
        .children()
        .map(|children| children.iter().map(|c| *c.id().as_bytes()).collect())
        .unwrap_or_default();

    if children_ids.is_empty() {
        // Leaf node
        if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
            let crdt_type = index.metadata.crdt_type.clone().ok_or_else(|| {
                eyre::eyre!(
                    "Missing CRDT type metadata for leaf entity {}: data integrity issue",
                    entity_id
                )
            })?;
            let metadata = LeafMetadata::new(crdt_type, index.metadata.updated_at(), [0u8; 32]);
            let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);
            Ok(Some(TreeNode::leaf(
                *entity_id.as_bytes(),
                full_hash,
                leaf_data,
            )))
        } else {
            Ok(Some(TreeNode::internal(
                *entity_id.as_bytes(),
                full_hash,
                vec![],
            )))
        }
    } else {
        Ok(Some(TreeNode::internal(
            *entity_id.as_bytes(),
            full_hash,
            children_ids,
        )))
    }
}

/// Apply leaf data using CRDT merge (Invariant I5).
///
/// This function must be called within a `with_runtime_env` scope.
/// Uses `Interface::apply_action` to properly update both the raw storage
/// and the Merkle tree Index.
fn apply_leaf_with_crdt_merge(context_id: ContextId, leaf: &TreeLeafData) -> Result<()> {
    use calimero_storage::entities::{ChildInfo, Metadata};
    use calimero_storage::interface::Action;

    let entity_id = Id::new(leaf.key);
    let root_id = Id::new(*context_id.as_ref());

    // Check if entity already exists
    let existing_index = Index::<MainStorage>::get_index(entity_id).ok().flatten();

    // Build metadata from leaf info
    let mut metadata = Metadata::default();
    metadata.crdt_type = Some(leaf.metadata.crdt_type.clone());
    metadata.updated_at = leaf.metadata.hlc_timestamp.into();

    let action = if existing_index.is_some() {
        // Update existing entity
        Action::Update {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![], // No ancestors needed for update
            metadata,
        }
    } else {
        // Add new entity as child of root
        // First ensure root exists
        if Index::<MainStorage>::get_index(root_id)
            .ok()
            .flatten()
            .is_none()
        {
            let root_action = Action::Update {
                id: root_id,
                data: vec![],
                ancestors: vec![],
                metadata: Metadata::default(),
            };
            Interface::<MainStorage>::apply_action(root_action)?;
        }

        // Get root info for ancestor chain
        let root_hash = Index::<MainStorage>::get_hashes_for(root_id)
            .ok()
            .flatten()
            .map(|(full, _)| full)
            .unwrap_or([0; 32]);
        let root_metadata = Index::<MainStorage>::get_index(root_id)
            .ok()
            .flatten()
            .map(|idx| idx.metadata.clone())
            .unwrap_or_default();

        let ancestor = ChildInfo::new(root_id, root_hash, root_metadata);

        Action::Add {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![ancestor],
            metadata,
        }
    };

    Interface::<MainStorage>::apply_action(action)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let config = HashComparisonConfig {
            remote_root_hash: [1u8; 32],
        };
        assert_eq!(config.remote_root_hash, [1u8; 32]);
    }

    #[test]
    fn test_stats_default() {
        let stats = HashComparisonStats::default();
        assert_eq!(stats.nodes_compared, 0);
        assert_eq!(stats.entities_merged, 0);
    }
}
