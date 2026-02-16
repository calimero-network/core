//! LevelWise sync protocol implementation (CIP Appendix B).
//!
//! Implements level-by-level breadth-first synchronization optimized for
//! wide, shallow trees (depth ≤ 2).
//!
//! # When to Use
//!
//! - `max_depth <= 2` (shallow trees)
//! - `avg_children_per_level > 10` (wide trees)
//! - Changes scattered across siblings
//!
//! # Algorithm
//!
//! ```text
//! 1. Request level 0 (root's children)
//! 2. Compare hashes with local via compare_level_nodes()
//! 3. For differing nodes:
//!    - If leaf → receive & CRDT merge entity
//!    - If internal → add to next_level_ids
//! 4. Request level 1 with parent_ids = differing internal nodes
//! 5. Continue until no more levels or max_depth reached
//! ```
//!
//! # Trade-offs
//!
//! | Aspect        | HashComparison     | LevelWise            |
//! |---------------|--------------------|-----------------------|
//! | Round trips   | O(depth)           | O(depth)              |
//! | Messages/round| 1                  | Batched by level      |
//! | Best for      | Deep trees         | Wide shallow trees    |
//!
//! # Usage
//!
//! ```ignore
//! use calimero_node::sync::level_sync::{LevelWiseProtocol, LevelWiseFirstRequest};
//! use calimero_node_primitives::sync::SyncProtocolExecutor;
//!
//! // Initiator side
//! let stats = LevelWiseProtocol::run_initiator(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     LevelWiseConfig { remote_root_hash, max_depth: 2 },
//! ).await?;
//!
//! // Responder side (manager extracts first request data)
//! let first_request = LevelWiseFirstRequest { level: 0, parent_ids: None };
//! LevelWiseProtocol::run_responder(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     first_request,
//! ).await?;
//! ```

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use calimero_node_primitives::sync::{
    compare_level_nodes, create_runtime_env, InitPayload, LevelNode, LevelWiseResponse,
    MessagePayload, StreamMessage, SyncProtocolExecutor, SyncTransport, TreeLeafData,
    MAX_LEVELWISE_DEPTH, MAX_NODES_PER_LEVEL, MAX_PARENTS_PER_REQUEST, MAX_REQUESTS_PER_SESSION,
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

use crate::sync::helpers::{apply_leaf_with_crdt_merge, generate_nonce};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for LevelWise initiator.
#[derive(Debug, Clone)]
pub struct LevelWiseConfig {
    /// Remote peer's root hash (from handshake).
    pub remote_root_hash: [u8; 32],
    /// Maximum depth to traverse (from protocol negotiation).
    pub max_depth: u32,
}

/// Data from the first `LevelWiseRequest` for responder dispatch.
///
/// The manager extracts this from the first `InitPayload::LevelWiseRequest`
/// and passes it to `run_responder`. This is necessary because the manager
/// consumes the first message for routing.
#[derive(Debug, Clone)]
pub struct LevelWiseFirstRequest {
    /// The level being requested (0 = root's children).
    pub level: u32,
    /// Parent node IDs to query children for (None = query from root).
    pub parent_ids: Option<Vec<[u8; 32]>>,
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics from a LevelWise sync session.
///
/// These stats can be used by the SyncManager to record metrics via
/// `SyncMetricsCollector` trait methods:
/// - `requests_sent` → `record_round_trip("LevelWise")`
/// - `entities_merged` → `record_entities_transferred(count)`
/// - `nodes_compared` → `record_message_sent("LevelWise", bytes)`
#[derive(Debug, Default, Clone)]
pub struct LevelWiseStats {
    /// Number of levels synced.
    pub levels_synced: u32,
    /// Number of tree nodes compared.
    pub nodes_compared: u64,
    /// Number of leaf entities merged via CRDT.
    pub entities_merged: u64,
    /// Number of nodes skipped (hashes matched).
    pub nodes_skipped: u64,
    /// Maximum nodes seen in a single level.
    pub max_nodes_per_level: usize,
    /// Number of requests sent to peer.
    pub requests_sent: u64,
    /// Whether final root hash was verified against expected.
    pub root_hash_verified: bool,
}

// =============================================================================
// Protocol Implementation
// =============================================================================

/// LevelWise sync protocol.
///
/// Implements breadth-first tree traversal for wide, shallow trees.
pub struct LevelWiseProtocol;

#[async_trait(?Send)]
impl SyncProtocolExecutor for LevelWiseProtocol {
    type Config = LevelWiseConfig;
    type ResponderInit = LevelWiseFirstRequest;
    type Stats = LevelWiseStats;

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
            config.max_depth,
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
            first_request.level,
            first_request.parent_ids,
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
    max_depth: u32,
) -> Result<LevelWiseStats> {
    info!(
        %context_id,
        max_depth,
        remote_root = %hex::encode(&remote_root_hash[..8]),
        "Starting LevelWise sync (initiator)"
    );

    let mut stats = LevelWiseStats::default();

    // Set up storage bridge
    let runtime_env = create_runtime_env(store, context_id, identity);

    // Track which parent IDs to query at next level
    // Start with None = request all nodes at level 0 (root's children)
    let mut current_parent_ids: Option<Vec<[u8; 32]>> = None;
    let clamped_max_depth = max_depth.min(MAX_LEVELWISE_DEPTH as u32);

    for level in 0..clamped_max_depth {
        // Build request for this level
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            payload: InitPayload::LevelWiseRequest {
                context_id,
                level,
                parent_ids: current_parent_ids.clone(),
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
            bail!("Expected Message, got {:?}", response);
        };

        let (resp_level, mut nodes, has_more_levels) = match payload {
            MessagePayload::LevelWiseResponse {
                level: resp_level,
                nodes,
                has_more_levels,
            } => (resp_level, nodes, has_more_levels),
            MessagePayload::SnapshotError { error } => {
                warn!(%context_id, ?error, "Peer returned error");
                bail!("Peer error: {:?}", error);
            }
            _ => bail!("Unexpected payload type, expected LevelWiseResponse"),
        };

        // DoS protection: validate response
        if resp_level != level {
            warn!(
                %context_id,
                expected = level,
                received = resp_level,
                "Level mismatch in response"
            );
            bail!("Level mismatch: expected {}, got {}", level, resp_level);
        }

        if nodes.len() > MAX_NODES_PER_LEVEL {
            warn!(
                %context_id,
                count = nodes.len(),
                max = MAX_NODES_PER_LEVEL,
                "Response too large"
            );
            bail!(
                "Response too large: {} nodes exceeds limit {}",
                nodes.len(),
                MAX_NODES_PER_LEVEL
            );
        }

        // Filter out invalid nodes in-place to avoid reallocation
        let original_count = nodes.len();
        nodes.retain(|node| node.is_valid());
        let invalid_count = original_count - nodes.len();
        if invalid_count > 0 {
            // Log once with count to avoid flooding logs with per-node warnings
            warn!(
                %context_id,
                invalid_count,
                original = original_count,
                valid = nodes.len(),
                "Filtered out invalid LevelNodes from response"
            );
        }

        stats.levels_synced = level + 1;
        stats.max_nodes_per_level = stats.max_nodes_per_level.max(nodes.len());

        debug!(
            %context_id,
            level,
            nodes_received = nodes.len(),
            has_more_levels,
            "Received level response"
        );

        if nodes.is_empty() {
            debug!(%context_id, level, "No nodes at this level, sync complete");
            break;
        }

        // Get local hashes for comparison
        let local_hashes = with_runtime_env(runtime_env.clone(), || {
            get_local_hashes_at_level(context_id, current_parent_ids.as_deref())
        })?;

        // Build response for comparison - move nodes in, then move them back out
        // to avoid expensive clone of potentially large leaf data
        let remote_response = LevelWiseResponse::new(level as usize, nodes, has_more_levels);

        // Compare local vs remote
        let compare_result = compare_level_nodes(&local_hashes, &remote_response);

        stats.nodes_compared += compare_result.total_compared() as u64;
        stats.nodes_skipped += compare_result.matching.len() as u64;

        debug!(
            %context_id,
            level,
            matching = compare_result.matching.len(),
            differing = compare_result.differing.len(),
            local_missing = compare_result.local_missing.len(),
            remote_missing = compare_result.remote_missing.len(),
            "Level comparison result"
        );

        // Move nodes back out of the response for subsequent processing
        let nodes = remote_response.nodes;

        // Process nodes that need sync
        let mut next_level_parents: Vec<[u8; 32]> = Vec::new();
        // Track already-added parent IDs to avoid duplicates - O(1) membership checks
        let mut added_parents: HashSet<[u8; 32]> = HashSet::new();

        // Build HashMap for O(1) node lookups instead of O(n) linear search
        // Use entry().or_insert() to keep first occurrence, consistent with compare_level_nodes
        let mut nodes_by_id: HashMap<[u8; 32], &LevelNode> = HashMap::new();
        for node in &nodes {
            nodes_by_id.entry(node.id).or_insert(node);
        }

        // Process differing and locally missing nodes
        // (nodes_to_process() includes both differing and local_missing)
        for node_id in compare_result.nodes_to_process() {
            // Find the node in the response - O(1) lookup
            let Some(node) = nodes_by_id.get(&node_id) else {
                continue;
            };

            if node.is_leaf() {
                // Leaf: apply CRDT merge (Invariant I5)
                if let Some(ref leaf_data) = node.leaf_data {
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
                // Internal node: add to next level query (avoid duplicates with O(1) check)
                if added_parents.insert(node.id) {
                    next_level_parents.push(node.id);
                }
            }
        }

        if !has_more_levels || next_level_parents.is_empty() {
            debug!(
                %context_id,
                level,
                "No more levels to sync"
            );
            break;
        }

        // Clamp parent IDs for next request (DoS protection)
        if next_level_parents.len() > MAX_PARENTS_PER_REQUEST {
            warn!(
                %context_id,
                count = next_level_parents.len(),
                max = MAX_PARENTS_PER_REQUEST,
                "Truncating parent IDs for next level request"
            );
            next_level_parents.truncate(MAX_PARENTS_PER_REQUEST);
        }

        current_parent_ids = Some(next_level_parents);
    }

    // Close the transport to signal completion
    transport.close().await?;

    // Verify root hash after sync to confirm convergence
    // This is a post-sync verification, not the Invariant I7 pre-write check
    let local_root_hash =
        with_runtime_env(runtime_env.clone(), || get_local_root_hash(context_id))?;

    stats.root_hash_verified = local_root_hash == remote_root_hash;

    if stats.root_hash_verified {
        debug!(
            %context_id,
            root_hash = %hex::encode(&local_root_hash[..8]),
            "Root hash verified after sync"
        );
    } else {
        // This is expected if we had local-only changes or CRDT merge produced different result
        // It's a warning, not an error, because CRDT merge may legitimately produce different hashes
        debug!(
            %context_id,
            local_hash = %hex::encode(&local_root_hash[..8]),
            remote_hash = %hex::encode(&remote_root_hash[..8]),
            "Root hash differs after sync (expected if local had concurrent changes)"
        );
    }

    info!(
        %context_id,
        levels_synced = stats.levels_synced,
        nodes_compared = stats.nodes_compared,
        entities_merged = stats.entities_merged,
        nodes_skipped = stats.nodes_skipped,
        max_nodes_per_level = stats.max_nodes_per_level,
        root_hash_verified = stats.root_hash_verified,
        "LevelWise sync complete"
    );

    Ok(stats)
}

// =============================================================================
// Responder Implementation
// =============================================================================

/// Run the LevelWise responder with the first request data.
///
/// The manager has already consumed the first `InitPayload::LevelWiseRequest`
/// for routing, so it passes the extracted `level` and `parent_ids` here.
async fn run_responder_impl<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    identity: PublicKey,
    first_level: u32,
    first_parent_ids: Option<Vec<[u8; 32]>>,
) -> Result<()> {
    info!(%context_id, "Starting LevelWise sync (responder)");

    // Set up storage bridge
    let runtime_env = create_runtime_env(store, context_id, identity);

    let mut sequence_id = 0u64;

    // Handle the first request (already parsed by the manager)
    let (nodes, has_more_levels) =
        handle_levelwise_request(context_id, first_level, first_parent_ids, &runtime_env)?;

    debug!(
        %context_id,
        level = first_level,
        nodes_found = nodes.len(),
        has_more_levels,
        "Responding with first level nodes"
    );

    let response = StreamMessage::Message {
        sequence_id,
        payload: MessagePayload::LevelWiseResponse {
            level: first_level,
            nodes,
            has_more_levels,
        },
        next_nonce: generate_nonce(),
    };
    transport.send(&response).await?;
    sequence_id += 1;

    // Handle subsequent requests in a loop
    run_responder_loop(transport, context_id, &runtime_env, sequence_id, 1).await
}

/// Handle a single LevelWise request and return the response data.
fn handle_levelwise_request(
    context_id: ContextId,
    level: u32,
    parent_ids: Option<Vec<[u8; 32]>>,
    runtime_env: &calimero_storage::env::RuntimeEnv,
) -> Result<(Vec<LevelNode>, bool)> {
    trace!(
        %context_id,
        level,
        parent_count = parent_ids.as_ref().map(|p| p.len()),
        "Handling LevelWiseRequest"
    );

    // DoS protection: validate request
    if level > MAX_LEVELWISE_DEPTH as u32 {
        warn!(
            %context_id,
            level,
            max = MAX_LEVELWISE_DEPTH,
            "Level exceeds maximum"
        );
        // Return empty response rather than error to avoid leaking state
        return Ok((vec![], false));
    }

    // DoS protection: truncate parent_ids if too large
    let truncated_parent_ids = parent_ids.map(|mut parents| {
        if parents.len() > MAX_PARENTS_PER_REQUEST {
            warn!(
                %context_id,
                count = parents.len(),
                max = MAX_PARENTS_PER_REQUEST,
                "Too many parent IDs in request, truncating"
            );
            parents.truncate(MAX_PARENTS_PER_REQUEST);
        }
        parents
    });

    // Get nodes at requested level
    with_runtime_env(runtime_env.clone(), || {
        get_nodes_at_level(context_id, level as usize, truncated_parent_ids.as_deref())
    })
}

/// Internal loop to handle subsequent LevelWise requests.
async fn run_responder_loop<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    mut sequence_id: u64,
    initial_requests_handled: u64,
) -> Result<()> {
    let mut requests_handled = initial_requests_handled;

    // Handle requests until stream closes or limit reached
    loop {
        // DoS protection: limit total requests per session
        if requests_handled >= MAX_REQUESTS_PER_SESSION {
            warn!(
                %context_id,
                requests_handled,
                max = MAX_REQUESTS_PER_SESSION,
                "Request limit reached, closing responder"
            );
            break;
        }

        let Some(request) = transport.recv().await? else {
            debug!(%context_id, requests_handled, "Stream closed, responder done");
            break;
        };

        let StreamMessage::Init { payload, .. } = request else {
            debug!(%context_id, "Received non-Init message, ending responder");
            break;
        };

        let InitPayload::LevelWiseRequest {
            level, parent_ids, ..
        } = payload
        else {
            debug!(%context_id, "Received non-LevelWiseRequest, ending responder");
            break;
        };

        let (nodes, has_more_levels) =
            handle_levelwise_request(context_id, level, parent_ids, runtime_env)?;

        debug!(
            %context_id,
            level,
            nodes_found = nodes.len(),
            has_more_levels,
            "Responding with level nodes"
        );

        // Send response
        let response = StreamMessage::Message {
            sequence_id,
            payload: MessagePayload::LevelWiseResponse {
                level,
                nodes,
                has_more_levels,
            },
            next_nonce: generate_nonce(),
        };

        transport.send(&response).await?;
        sequence_id += 1;
        requests_handled += 1;
    }

    info!(%context_id, requests_handled, "LevelWise responder complete");
    Ok(())
}

// =============================================================================
// Storage Helpers
// =============================================================================

/// Get the local root hash for verification.
fn get_local_root_hash(context_id: ContextId) -> Result<[u8; 32]> {
    let root_id = Id::new(*context_id.as_ref());

    match Index::<MainStorage>::get_hashes_for(root_id) {
        Ok(Some((full_hash, _))) => Ok(full_hash),
        Ok(None) => Ok([0u8; 32]), // Empty tree has zero hash
        Err(e) => {
            warn!(%context_id, error = %e, "Failed to get root hash");
            Ok([0u8; 32])
        }
    }
}

/// Get local node hashes at a level for comparison.
///
/// Returns a map of node_id -> hash for all nodes at the specified level.
fn get_local_hashes_at_level(
    context_id: ContextId,
    parent_ids: Option<&[[u8; 32]]>,
) -> Result<HashMap<[u8; 32], [u8; 32]>> {
    let mut hashes = HashMap::new();

    let root_id = Id::new(*context_id.as_ref());

    // Get the root index to access children
    let root_index = match Index::<MainStorage>::get_index(root_id) {
        Ok(Some(idx)) => idx,
        Ok(None) => return Ok(hashes), // Empty tree
        Err(e) => {
            warn!(%context_id, error = %e, "Failed to get root index");
            return Ok(hashes);
        }
    };

    match parent_ids {
        None => {
            // Level 0: get direct children of root
            if let Some(children) = root_index.children() {
                for child in children {
                    let child_id = *child.id().as_bytes();
                    if let Some(child_hash) = Index::<MainStorage>::get_hashes_for(child.id())
                        .ok()
                        .flatten()
                    {
                        hashes.insert(child_id, child_hash.0);
                    }
                }
            }
        }
        Some(parents) => {
            // Deeper levels: get children of specified parents
            for parent_id in parents {
                let parent_storage_id = Id::new(*parent_id);
                if let Ok(Some(parent_index)) = Index::<MainStorage>::get_index(parent_storage_id) {
                    if let Some(children) = parent_index.children() {
                        for child in children {
                            let child_id = *child.id().as_bytes();
                            if let Some(child_hash) =
                                Index::<MainStorage>::get_hashes_for(child.id())
                                    .ok()
                                    .flatten()
                            {
                                hashes.insert(child_id, child_hash.0);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(hashes)
}

/// Get nodes at a level for responding to LevelWiseRequest.
///
/// Returns nodes at the level and whether there are more levels below.
fn get_nodes_at_level(
    context_id: ContextId,
    level: usize,
    parent_ids: Option<&[[u8; 32]]>,
) -> Result<(Vec<LevelNode>, bool)> {
    let mut nodes = Vec::new();
    let mut has_more_levels = false;

    let root_id = Id::new(*context_id.as_ref());

    // Verify root exists before proceeding
    match Index::<MainStorage>::get_index(root_id) {
        Ok(Some(_)) => {}                      // Root exists, continue
        Ok(None) => return Ok((nodes, false)), // Empty tree
        Err(e) => {
            warn!(%context_id, error = %e, "Failed to get root index");
            return Ok((nodes, false));
        }
    }

    // Collect parent nodes to query
    let parents_to_query: Vec<Id> = match parent_ids {
        None if level == 0 => {
            // Level 0: query root's children
            vec![root_id]
        }
        None => {
            // This shouldn't happen - deeper levels need parent_ids
            warn!(%context_id, level, "No parent_ids for level > 0");
            return Ok((nodes, false));
        }
        Some(ids) => ids.iter().map(|id| Id::new(*id)).collect(),
    };

    for parent_id in parents_to_query {
        let parent_index = match Index::<MainStorage>::get_index(parent_id) {
            Ok(Some(idx)) => idx,
            Ok(None) => continue,
            Err(_) => continue,
        };

        let Some(children) = parent_index.children() else {
            continue;
        };

        for child in children {
            let child_storage_id = child.id();
            let child_id = *child_storage_id.as_bytes();

            // Get child's index for hash and to determine if leaf/internal
            let child_index = match Index::<MainStorage>::get_index(child_storage_id) {
                Ok(Some(idx)) => idx,
                Ok(None) => continue,
                Err(_) => continue,
            };

            let child_hash = child_index.full_hash();
            let is_leaf = child_index.children().is_none()
                || child_index.children().map(|c| c.is_empty()).unwrap_or(true);

            // Determine parent_id for this node (None for level 0)
            let parent_id_bytes = if level == 0 {
                None
            } else {
                Some(*parent_id.as_bytes())
            };

            if is_leaf {
                // Get leaf data for CRDT merge
                if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(child_storage_id)
                {
                    let crdt_type = child_index.metadata.crdt_type.clone().ok_or_else(|| {
                        eyre::eyre!(
                            "Missing CRDT type metadata for leaf entity {}: data integrity issue",
                            child_storage_id
                        )
                    })?;

                    let metadata = calimero_node_primitives::sync::LeafMetadata::new(
                        crdt_type,
                        child_index.metadata.updated_at(),
                        [0u8; 32],
                    );
                    let leaf_data =
                        TreeLeafData::new(*child_storage_id.as_bytes(), entry_data, metadata);

                    nodes.push(LevelNode::leaf(
                        child_id,
                        child_hash,
                        parent_id_bytes,
                        leaf_data,
                    ));
                } else {
                    // Leaf node with no raw data is corrupted/incomplete - skip it
                    debug!(
                        %context_id,
                        child_id = %hex::encode(&child_id[..8]),
                        "Skipping leaf node with no raw data"
                    );
                    continue;
                }
            } else {
                // Internal node: has children, so more levels exist
                has_more_levels = true;
                nodes.push(LevelNode::internal(child_id, child_hash, parent_id_bytes));
            }

            // DoS protection: limit nodes
            if nodes.len() >= MAX_NODES_PER_LEVEL {
                warn!(
                    %context_id,
                    level,
                    "Reached maximum nodes per level"
                );
                break;
            }
        }

        if nodes.len() >= MAX_NODES_PER_LEVEL {
            break;
        }
    }

    Ok((nodes, has_more_levels))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let config = LevelWiseConfig {
            remote_root_hash: [1u8; 32],
            max_depth: 2,
        };
        assert_eq!(config.remote_root_hash, [1u8; 32]);
        assert_eq!(config.max_depth, 2);
    }

    #[test]
    fn test_stats_default() {
        let stats = LevelWiseStats::default();
        assert_eq!(stats.levels_synced, 0);
        assert_eq!(stats.nodes_compared, 0);
        assert_eq!(stats.entities_merged, 0);
        assert_eq!(stats.nodes_skipped, 0);
        assert_eq!(stats.max_nodes_per_level, 0);
        assert_eq!(stats.requests_sent, 0);
        assert!(!stats.root_hash_verified);
    }

    #[test]
    fn test_stats_tracking() {
        let mut stats = LevelWiseStats::default();
        stats.levels_synced = 2;
        stats.nodes_compared = 100;
        stats.entities_merged = 25;
        stats.nodes_skipped = 75;
        stats.max_nodes_per_level = 50;
        stats.requests_sent = 3;
        stats.root_hash_verified = true;

        assert_eq!(stats.levels_synced, 2);
        assert_eq!(stats.nodes_compared, 100);
        assert_eq!(stats.entities_merged, 25);
        assert_eq!(stats.nodes_skipped, 75);
        assert_eq!(stats.max_nodes_per_level, 50);
        assert_eq!(stats.requests_sent, 3);
        assert!(stats.root_hash_verified);
    }
}
