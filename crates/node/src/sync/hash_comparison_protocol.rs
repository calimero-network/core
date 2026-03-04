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
//! use calimero_node::sync::hash_comparison_protocol::{
//!     HashComparisonProtocol, HashComparisonFirstRequest
//! };
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
//! // Responder side (manager extracts first request data)
//! let first_request = HashComparisonFirstRequest { node_id, max_depth: Some(1) };
//! HashComparisonProtocol::run_responder(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     first_request,
//! ).await?;
//! ```

use crate::sync::helpers::{
    apply_leaf_with_crdt_merge, generate_nonce, handle_entity_push, MAX_ENTITIES_PER_PUSH,
};
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

/// Maximum requests allowed per HashComparison session.
///
/// Prevents DoS by limiting how many requests a peer can send.
///
/// This limit is higher than LevelWise's `MAX_REQUESTS_PER_SESSION` (128) because:
/// - HashComparison uses DFS traversal, one request per tree node
/// - LevelWise uses BFS traversal, one request per tree level
/// - For a tree with N nodes, HashComparison needs O(N) requests vs O(depth) for LevelWise
///
/// With 10,000 requests and typical node sizes, this allows syncing trees up to ~10k entities.
pub const MAX_HASH_COMPARISON_REQUESTS: u64 = 10_000;

/// Configuration for HashComparison initiator.
#[derive(Debug, Clone)]
pub struct HashComparisonConfig {
    /// Remote peer's root hash (from handshake).
    pub remote_root_hash: [u8; 32],
}

/// Data from the first `TreeNodeRequest` for responder dispatch.
///
/// The manager extracts this from the first `InitPayload::TreeNodeRequest`
/// and passes it to `run_responder`. This is necessary because the manager
/// consumes the first message for routing.
#[derive(Debug, Clone)]
pub struct HashComparisonFirstRequest {
    /// The node ID being requested.
    pub node_id: [u8; 32],
    /// Maximum depth to return children.
    pub max_depth: Option<u8>,
}

/// Statistics from a HashComparison sync session.
#[derive(Debug, Default, Clone)]
pub struct HashComparisonStats {
    /// Number of tree nodes compared.
    pub nodes_compared: u64,
    /// Number of leaf entities merged via CRDT (pulled from peer).
    pub entities_merged: u64,
    /// Number of leaf entities pushed to peer (bidirectional sync).
    pub entities_pushed: u64,
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
    type ResponderInit = HashComparisonFirstRequest;
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
        first_request: Self::ResponderInit,
    ) -> Result<()> {
        run_responder_impl(
            transport,
            store,
            context_id,
            identity,
            first_request.node_id,
            first_request.max_depth,
        )
        .await
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
                        local_only_children,
                        common_children,
                    } => {
                        // Recurse into remote-only and common children (pull)
                        for child_id in remote_only_children {
                            to_compare.push((child_id, false));
                        }
                        for child_id in common_children {
                            to_compare.push((child_id, false));
                        }

                        // Bidirectional: push local-only subtrees to peer
                        if !local_only_children.is_empty() {
                            let pushed = push_local_subtrees(
                                transport,
                                &runtime_env,
                                context_id,
                                identity,
                                &local_only_children,
                                &mut stats,
                            )
                            .await?;
                            debug!(
                                %context_id,
                                local_only = local_only_children.len(),
                                entities_pushed = pushed,
                                "Pushed local-only children to peer"
                            );
                        }
                    }
                    TreeCompareResult::RemoteMissing => {
                        // Bidirectional: the initiator has this entire subtree
                        // but the remote doesn't. Push all leaf data.
                        if let Some(ref local_node) = local_version {
                            let leaves = with_runtime_env(runtime_env.clone(), || {
                                collect_local_leaves(context_id, &local_node.id, false)
                            })?;
                            if !leaves.is_empty() {
                                push_entities(transport, context_id, identity, &leaves, &mut stats)
                                    .await?;
                            }
                        }
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
        entities_pushed = stats.entities_pushed,
        nodes_skipped = stats.nodes_skipped,
        "HashComparison sync complete"
    );

    Ok(stats)
}

// =============================================================================
// Responder Implementation
// =============================================================================

/// Run the HashComparison responder with the first request data.
///
/// The manager has already consumed the first `InitPayload::TreeNodeRequest`
/// for routing, so it passes the extracted `node_id` and `max_depth` here.
async fn run_responder_impl<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    identity: PublicKey,
    first_node_id: [u8; 32],
    first_max_depth: Option<u8>,
) -> Result<()> {
    info!(%context_id, "Starting HashComparison sync (responder)");

    // Defense in depth: validate first request parameters
    // (The manager should have validated these, but we check again)
    if let Some(depth) = first_max_depth {
        if depth > MAX_REQUEST_DEPTH {
            bail!(
                "First request max_depth {} exceeds maximum {}",
                depth,
                MAX_REQUEST_DEPTH
            );
        }
    }

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

    // Handle the first request (already parsed by the manager)
    {
        let clamped_depth = first_max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
        let is_root_request = first_node_id == local_root_hash;

        let local_node = with_runtime_env(runtime_env.clone(), || {
            get_local_tree_node(context_id, &first_node_id, is_root_request)
        })?;

        let response =
            build_tree_node_response_internal(context_id, local_node, clamped_depth, &runtime_env)?;

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

    // Handle subsequent requests until stream closes or limit reached
    loop {
        // DoS protection: limit total requests per session
        if requests_handled >= MAX_HASH_COMPARISON_REQUESTS {
            warn!(
                %context_id,
                requests_handled,
                max = MAX_HASH_COMPARISON_REQUESTS,
                "Request limit reached, closing responder"
            );
            break;
        }

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

        match payload {
            InitPayload::TreeNodeRequest {
                node_id, max_depth, ..
            } => {
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

                let response = build_tree_node_response_internal(
                    context_id,
                    local_node,
                    clamped_depth,
                    &runtime_env,
                )?;

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

            InitPayload::EntityPush { entities, .. } => {
                let entity_count = entities.len();
                trace!(%context_id, entity_count, "Handling EntityPush from initiator");

                let applied = handle_entity_push(&runtime_env, context_id, &entities);

                let msg = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::EntityPushAck {
                        applied_count: applied,
                    },
                    next_nonce: generate_nonce(),
                };

                transport.send(&msg).await?;
                sequence_id += 1;
                requests_handled += 1;

                info!(
                    %context_id,
                    applied,
                    total = entity_count,
                    "Applied pushed entities via CRDT merge"
                );
            }

            _ => {
                // Unknown payload type - end responder
                debug!(%context_id, "Received unknown payload, ending responder");
                break;
            }
        }
    }

    info!(%context_id, requests_handled, "HashComparison responder complete");
    Ok(())
}

/// Build a TreeNodeResponse from a local node.
fn build_tree_node_response_internal(
    context_id: ContextId,
    local_node: Option<TreeNode>,
    clamped_depth: Option<u8>,
    runtime_env: &calimero_storage::env::RuntimeEnv,
) -> Result<TreeNodeResponse> {
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

    Ok(response)
}

// =============================================================================
// Bidirectional Sync: Push Helpers
// =============================================================================

/// Maximum recursion depth for collecting leaves from a subtree.
///
/// Prevents stack overflow from deeply nested or corrupted trees.
const MAX_COLLECT_DEPTH: u32 = 64;

/// Maximum leaves to collect from a single subtree.
///
/// Prevents unbounded memory growth for very wide trees.
/// Matches `MAX_HASH_COMPARISON_REQUESTS` in scale.
const MAX_LEAVES_PER_SUBTREE: usize = 10_000;

/// Collect all leaf entities from a local subtree recursively.
///
/// Walks the Merkle tree starting from `node_id` and collects all leaf
/// entities (with their data and CRDT metadata). Used when the initiator
/// needs to push local-only data to the peer.
///
/// Capped at `MAX_LEAVES_PER_SUBTREE` to prevent unbounded memory growth.
///
/// Must be called within a `with_runtime_env` scope.
fn collect_local_leaves(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root: bool,
) -> Result<Vec<TreeLeafData>> {
    let mut leaves = Vec::new();
    collect_leaves_recursive(context_id, node_id, is_root, &mut leaves, 0)?;
    Ok(leaves)
}

/// Recursively collect leaf data from a subtree.
fn collect_leaves_recursive(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root: bool,
    leaves: &mut Vec<TreeLeafData>,
    depth: u32,
) -> Result<()> {
    if depth >= MAX_COLLECT_DEPTH {
        warn!(
            depth,
            node_id = %hex::encode(node_id),
            "collect_leaves_recursive: max depth reached, truncating"
        );
        return Ok(());
    }

    if leaves.len() >= MAX_LEAVES_PER_SUBTREE {
        return Ok(());
    }
    let entity_id = if is_root {
        Id::new(*context_id.as_ref())
    } else {
        Id::new(*node_id)
    };

    let index = match Index::<MainStorage>::get_index(entity_id) {
        Ok(Some(idx)) => idx,
        Ok(None) => return Ok(()),
        Err(e) => {
            warn!(
                %entity_id,
                error = %e,
                "collect_leaves_recursive: failed to read index, skipping subtree"
            );
            return Ok(());
        }
    };

    let children_ids: Vec<[u8; 32]> = index
        .children()
        .map(|children| children.iter().map(|c| *c.id().as_bytes()).collect())
        .unwrap_or_default();

    if children_ids.is_empty() {
        // Leaf node — collect its data
        if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
            if let Some(ref crdt_type) = index.metadata.crdt_type {
                let metadata =
                    LeafMetadata::new(crdt_type.clone(), index.metadata.updated_at(), [0u8; 32]);
                let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);
                leaves.push(leaf_data);
            } else {
                warn!(
                    %entity_id,
                    "collect_leaves_recursive: leaf missing crdt_type, skipping"
                );
            }
        }
    } else {
        // Internal node — recurse into children
        for child_id in &children_ids {
            collect_leaves_recursive(context_id, child_id, false, leaves, depth + 1)?;
        }
    }

    Ok(())
}

/// Push local-only subtrees to the peer.
///
/// For each child ID in `local_only_children`, walks the local tree to
/// collect leaf data, then sends it to the peer via `EntityPush` messages.
async fn push_local_subtrees<T: SyncTransport>(
    transport: &mut T,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_id: ContextId,
    identity: PublicKey,
    local_only_children: &[[u8; 32]],
    stats: &mut HashComparisonStats,
) -> Result<u64> {
    let mut total = 0u64;

    // Flush per-subtree to avoid accumulating all leaves in memory
    for child_id in local_only_children {
        let leaves = with_runtime_env(runtime_env.clone(), || {
            collect_local_leaves(context_id, child_id, false)
        })?;
        if !leaves.is_empty() {
            total += push_entities(transport, context_id, identity, &leaves, stats).await?;
        }
    }

    Ok(total)
}

/// Send entities to the peer via `EntityPush` messages (batched).
///
/// Sends in batches of `MAX_ENTITIES_PER_PUSH` to avoid overly large messages.
async fn push_entities<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    identity: PublicKey,
    leaves: &[TreeLeafData],
    stats: &mut HashComparisonStats,
) -> Result<u64> {
    let mut total_pushed = 0u64;

    for chunk in leaves.chunks(MAX_ENTITIES_PER_PUSH) {
        let push_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            payload: InitPayload::EntityPush {
                context_id,
                entities: chunk.to_vec(),
            },
            next_nonce: generate_nonce(),
        };

        transport.send(&push_msg).await?;
        stats.requests_sent += 1;

        // Wait for acknowledgment
        let ack = transport
            .recv()
            .await?
            .ok_or_else(|| eyre::eyre!("stream closed while waiting for EntityPushAck"))?;

        match ack {
            StreamMessage::Message {
                payload: MessagePayload::EntityPushAck { applied_count },
                ..
            } => {
                total_pushed += u64::from(applied_count);
            }
            _ => {
                bail!(
                    "Unexpected response to EntityPush (peer may not support bidirectional sync)"
                );
            }
        }
    }

    stats.entities_pushed += total_pushed;
    Ok(total_pushed)
}

// =============================================================================
// Local Tree Node Lookup
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
