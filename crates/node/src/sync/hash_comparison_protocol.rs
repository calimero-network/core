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
    apply_leaf_with_crdt_merge, generate_nonce, get_local_root_hash_for_context,
    handle_entity_push, MAX_ENTITIES_PER_PUSH,
};
use async_trait::async_trait;
use calimero_node_primitives::sync::{
    compare_tree_nodes, create_runtime_env, InitPayload, LeafMetadata, MessagePayload,
    StreamMessage, SyncProtocolExecutor, SyncTransport, TreeCompareResult, TreeLeafData, TreeNode,
    TreeNodeResponse, MAX_LEAF_VALUE_SIZE, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
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

/// Synthetic `CrdtType::LwwRegister` inner-type name used on the wire for a
/// Merkle leaf whose stored `index.metadata.crdt_type` is `None` ("opaque"
/// leaf — e.g. the WASM app's `Root<T>` state entry `Id::new([118; 32])`).
///
/// The storage layer treats `crdt_type == None` and
/// `crdt_type == Some(LwwRegister { .. })` identically for merge (incoming
/// wins iff `updated_at >= existing`), and `crdt_type` is *not* an input to a
/// leaf's Merkle hash (`Metadata` is `#[borsh(skip)]` on `Element`), so
/// emitting a leaf with this synthetic type is wire-format-stable and
/// merge-equivalent to the `None` it stands in for. See
/// `docs/superpowers/specs/2026-05-13-opaque-leaf-sync-design.md`.
pub(super) const OPAQUE_LEAF_CRDT_TYPE_NAME: &str = "Opaque";

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
    /// Whether the post-sync local root hash matches the remote
    /// root hash the initiator started with. Set by the initiator
    /// after the DFS merge completes. A `false` value indicates the
    /// merge did not converge the two peers — see #2407 for the
    /// failure mode this guards against.
    pub root_hash_verified: bool,
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

    // Leaves the initiator needs to push back to the peer because local
    // is divergent from what the peer just sent us (see #2407
    // bidirectional reconciliation below). Collected during the DFS and
    // flushed in one chunked call after the walk to keep the number of
    // round-trips O(divergent_leaves / MAX_ENTITIES_PER_PUSH) rather
    // than O(divergent_leaves), and to keep `stats.requests_sent` well
    // below `MAX_HASH_COMPARISON_REQUESTS` on heavily-diverged trees.
    let mut pending_local_leaf_pushes: Vec<TreeLeafData> = Vec::new();

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
            if is_root_request {
                // #2407 root-advance race: the peer's root moved between
                // handshake (where we captured `remote_root_hash`) and now,
                // so the peer no longer has that exact internal-node entity
                // in its index. Without this branch the DFS stack stays
                // empty, the session closes cleanly, and `Ok(stats)` is
                // returned with all counters at zero — the manager records
                // it as a successful sync and the divergent node never
                // recovers. Bailing here surfaces the failure to the
                // manager's fallback chain (DAG catchup → snapshot), which
                // re-handshakes against the peer's current root.
                bail!(
                    "HashComparison sync aborted: peer reported root node not_found \
                     (peer's root advanced after handshake)"
                );
            }
            debug!(%context_id, node_id = %hex::encode(node_id), "Node not found on peer");
            continue;
        }

        // Process each node
        for (node_idx, remote_node) in nodes.into_iter().enumerate() {
            // #2319: the SyncSessionActor runs every session on one
            // arbiter thread, and `apply_leaf_with_crdt_merge` (the WASM
            // CRDT merge below) is synchronous with no await between
            // merges — a full 1000-leaf batch (MAX_NODES_PER_RESPONSE)
            // would pin the thread and stall the actor's mailbox. Yield
            // every 64 nodes so the actor can accept/drain queued jobs
            // mid-repair.
            if node_idx != 0 && node_idx % 64 == 0 {
                tokio::task::yield_now().await;
            }

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

                    // #2407 bidirectional leaf reconciliation: a parent's
                    // `children` list is keyed by entity_id (see
                    // `get_local_tree_node`: `c.id().as_bytes()`), so a
                    // same-entity-different-HLC divergence ends up in
                    // `common_children` and DFS recurses here. We've
                    // already pulled the peer's version above; the
                    // storage layer's LWW guard keeps the newer of
                    // {local, peer}. If local won (silent skip), the
                    // peer still holds the older version and would
                    // re-emit it on every subsequent sync — the sticky
                    // loop documented in #2407 evidence
                    // (`entities_merged=2, entities_pushed=0,
                    // success_count climbing forever`). Queue local's
                    // leaf to push back so the peer can converge in the
                    // SAME session; the peer's own LWW guard skips if
                    // its version is already newer, so this is a no-op
                    // when unnecessary. We accumulate and flush in one
                    // chunked batch after the DFS so an N-leaf
                    // divergence is N entities over O(N/batch) round-
                    // trips, not N round-trips inline.
                    let local_node = with_runtime_env(runtime_env.clone(), || {
                        get_local_tree_node(context_id, &remote_node.id, false)
                    })?;
                    if let Some(local) = local_node {
                        if local.is_leaf() && local.hash != remote_node.hash {
                            if let Some(local_leaf) = local.leaf_data {
                                // Same guard `collect_local_leaves`
                                // applies on the snapshot-push path:
                                // an oversized leaf is rejected by
                                // the peer's `TreeLeafData::is_valid`
                                // check inside `handle_entity_push`,
                                // so queuing it here would silently
                                // fail and re-enter the sticky loop
                                // this fix exists to eliminate.
                                if local_leaf.value.len() > MAX_LEAF_VALUE_SIZE {
                                    warn!(
                                        %context_id,
                                        key = %hex::encode(local_leaf.key),
                                        len = local_leaf.value.len(),
                                        max = MAX_LEAF_VALUE_SIZE,
                                        "leaf value exceeds MAX_LEAF_VALUE_SIZE, \
                                         skipping bidirectional push"
                                    );
                                } else {
                                    pending_local_leaf_pushes.push(local_leaf);
                                }
                            }
                        }
                    }
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
                                collect_local_leaves(context_id, &local_node.id, is_this_node_root)
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

        // #2319: yield once per peer round-trip batch too, in case the
        // batch was < 64 nodes but we are walking thousands of them.
        tokio::task::yield_now().await;
    }

    // Flush bidirectional-reconciliation leaf pushes (#2407). One
    // chunked `push_entities` call covers all divergent leaves
    // discovered during the DFS; the helper batches at
    // `MAX_ENTITIES_PER_PUSH` and a single EntityPushAck is consumed
    // per batch, so the request budget stays bounded.
    if !pending_local_leaf_pushes.is_empty() {
        let pushed = push_entities(
            transport,
            context_id,
            identity,
            &pending_local_leaf_pushes,
            &mut stats,
        )
        .await?;
        debug!(
            %context_id,
            divergent_leaves = pending_local_leaf_pushes.len(),
            entities_pushed = pushed,
            "Flushed bidirectional leaf reconciliation pushes"
        );
    }

    // Close the transport to signal completion to the responder
    transport.close().await?;

    // Post-sync convergence check (#2407). We compare the local
    // root against the remote root the initiator started with, but
    // do NOT treat mismatch as fatal:
    //
    // - Pull-only convergence: local should equal remote_root_hash.
    // - Bidirectional sync: the initiator pushed local-only data to
    //   the peer, so the peer's root advanced past
    //   `remote_root_hash` (which we captured at handshake) — the
    //   initiator's post-sync root won't match that stale value,
    //   even though both peers have converged to the same NEW root.
    // - Concurrent local writes between handshake and now: same
    //   shape — local moves on, won't match captured remote.
    //
    // We can't distinguish "real divergence bug (#2407)" from
    // "legitimate bidirectional drift" without a second handshake
    // round-trip. So instead: set the flag, surface mismatch at
    // WARN level (was: silent debug), and let the sync manager /
    // metrics consumer react to *patterns* of unverified syncs.
    // The bug #2407 documents is a node logging this WARN every
    // second forever — that's now visible in logs and via the
    // `root_hash_verified` stats field.
    let local_root_hash = with_runtime_env(runtime_env.clone(), || {
        get_local_root_hash_for_context(context_id)
    })?;
    stats.root_hash_verified = local_root_hash == remote_root_hash;

    info!(
        %context_id,
        nodes_compared = stats.nodes_compared,
        entities_merged = stats.entities_merged,
        entities_pushed = stats.entities_pushed,
        nodes_skipped = stats.nodes_skipped,
        root_hash_verified = stats.root_hash_verified,
        "HashComparison sync complete"
    );

    if !stats.root_hash_verified {
        warn!(
            %context_id,
            local_hash = %hex::encode(&local_root_hash[..8]),
            remote_hash = %hex::encode(&remote_root_hash[..8]),
            nodes_compared = stats.nodes_compared,
            entities_merged = stats.entities_merged,
            entities_pushed = stats.entities_pushed,
            nodes_skipped = stats.nodes_skipped,
            "HashComparison sync did not match remote handshake root (#2407). \
             Legitimate in bidirectional sync (peer's root advanced after our push) \
             or with concurrent local writes; persistent occurrences of this WARN \
             across many interval-sync ticks indicate a real merge convergence bug."
        );
    }

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

    if leaves.len() > MAX_LEAVES_PER_SUBTREE {
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
        // Leaf node — collect its data. Internal nodes (children non-empty)
        // are NOT emitted as leaves: storage-layer collection containers
        // have structural Merkle bytes in their `find_by_id_raw` result
        // (children list / `Collection` borsh) that aren't user data and
        // would corrupt the receiver if applied as a leaf. Pushing only
        // true leaves and reconstructing internal structure via parent_id
        // links on those leaves is the correct shape for this protocol.
        if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
            let crdt_type = index.metadata.crdt_type.clone().unwrap_or_else(|| {
                // Opaque leaf — carry it with a synthetic LWW wire type so it is
                // pushed (and is `is_valid()` on the peer), not silently dropped.
                trace!(%entity_id, "opaque leaf, synthesised LWW wire type for push");
                CrdtType::lww_register(OPAQUE_LEAF_CRDT_TYPE_NAME)
            });
            // Carry the leaf's Merkle parent_id on the wire so the
            // receiver can place the entity at the correct position in
            // *its* tree instead of always making it a direct child of
            // the context root. The receiver's apply path
            // (`apply_leaf_with_crdt_merge`) reads this back; pre-fix
            // the field was always `None` and the receiver fell back to
            // context-root, which silently corrupted the Merkle topology
            // for any nested-collection entity → divergent root hash
            // that HashComparison could never heal. See the smoke-test
            // Round-2 failure on bdc61af for evidence.
            let mut metadata = LeafMetadata::new(crdt_type, index.metadata.updated_at(), [0u8; 32])
                .with_created_at(index.metadata.created_at());
            if let Some(parent_id) = index.parent_id() {
                metadata = metadata.with_parent(*parent_id.as_bytes());
            }
            if let Some(auth) = crate::sync::helpers::wire_authorization_for(&index.metadata) {
                metadata = metadata.with_authorization(auth);
            }
            let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);
            if leaf_data.value.len() > MAX_LEAF_VALUE_SIZE {
                warn!(
                    %entity_id,
                    len = leaf_data.value.len(),
                    "leaf value exceeds MAX_LEAF_VALUE_SIZE, skipping push"
                );
            } else {
                leaves.push(leaf_data);
            }
        }
    } else {
        // Internal node — recurse into children. Their parent_id on the
        // wire identifies *this* entity as their parent, so the receiver
        // can rebuild the tree structure without needing this internal
        // node's bytes.
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
            let crdt_type = index.metadata.crdt_type.clone().unwrap_or_else(|| {
                // No CRDT type ("opaque" leaf — e.g. the `Root<T>` state entry).
                // Emit a real *leaf* (not a malformed empty `internal` node, which the
                // peer's `TreeNode::is_valid()` rejects) carrying a synthetic LWW wire
                // type — merge-equivalent to `None` and Merkle-hash-neutral.
                trace!(%entity_id, "opaque leaf, synthesised LWW wire type for sync");
                CrdtType::lww_register(OPAQUE_LEAF_CRDT_TYPE_NAME)
            });
            // Carry the leaf's Merkle parent_id on the wire — see the same
            // comment in `collect_leaves_recursive` for rationale.
            let mut metadata = LeafMetadata::new(crdt_type, index.metadata.updated_at(), [0u8; 32])
                .with_created_at(index.metadata.created_at());
            if let Some(parent_id) = index.parent_id() {
                metadata = metadata.with_parent(*parent_id.as_bytes());
            }
            if let Some(auth) = crate::sync::helpers::wire_authorization_for(&index.metadata) {
                metadata = metadata.with_authorization(auth);
            }
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

    /// An opaque (no-`crdt_type`) Merkle leaf must be emitted as a real *leaf*
    /// `TreeNode` (not a malformed empty `internal` node, which the peer drops as
    /// "Invalid TreeNode") carrying a synthetic LWW wire type.
    #[test]
    fn get_local_tree_node_returns_leaf_for_no_crdt_entity() {
        use std::sync::Arc;

        use calimero_storage::action::Action;
        use calimero_storage::entities::{ChildInfo, Metadata};
        use calimero_storage::interface::ApplyContext;
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;

        let context_id = ContextId::from([0xCA; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        // `Id::new([118; 32])` == `Root::<T>::entry_id()` — an opaque leaf.
        let root_id = Id::new(*context_id.as_ref());
        let opaque_id = Id::new([118u8; 32]);

        with_runtime_env(runtime_env.clone(), || {
            // Create the context root.
            Interface::<MainStorage>::apply_action(
                Action::Update {
                    id: root_id,
                    data: vec![],
                    ancestors: vec![],
                    metadata: Metadata::default(),
                },
                &ApplyContext::empty(),
            )
            .expect("create root");

            // Add the opaque leaf as a child of root — `Metadata::new` => crdt_type None.
            let root_hash = Index::<MainStorage>::get_hashes_for(root_id)
                .ok()
                .flatten()
                .map(|(full, _)| full)
                .unwrap_or([0; 32]);
            let root_meta = Index::<MainStorage>::get_index(root_id)
                .ok()
                .flatten()
                .map(|idx| idx.metadata.clone())
                .unwrap_or_default();
            Interface::<MainStorage>::apply_action(
                Action::Add {
                    id: opaque_id,
                    data: b"app-root-state".to_vec(),
                    ancestors: vec![ChildInfo::new(root_id, root_hash, root_meta)],
                    metadata: Metadata::new(100, 100),
                },
                &ApplyContext::empty(),
            )
            .expect("add opaque leaf");

            // Sanity: it really is opaque.
            assert!(
                Index::<MainStorage>::get_index(opaque_id)
                    .unwrap()
                    .unwrap()
                    .metadata
                    .crdt_type
                    .is_none(),
                "seeded entity must have crdt_type == None"
            );

            let node = get_local_tree_node(context_id, opaque_id.as_bytes(), false)
                .expect("get_local_tree_node should not error")
                .expect("node should exist");

            assert!(node.is_leaf(), "opaque entity must be emitted as a leaf");
            assert!(
                !node.is_internal(),
                "opaque entity must not be an internal node"
            );
            assert!(
                node.is_valid(),
                "opaque leaf node must be structurally valid"
            );
            let leaf_data = node.leaf_data.as_ref().expect("leaf must carry leaf_data");
            assert!(
                matches!(leaf_data.metadata.crdt_type, CrdtType::LwwRegister { .. }),
                "opaque leaf must carry a synthetic LwwRegister wire type, got {:?}",
                leaf_data.metadata.crdt_type
            );
            assert_eq!(leaf_data.value, b"app-root-state");
        });
    }
}
