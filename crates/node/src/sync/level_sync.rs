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
//! 3. For differing nodes, independently:
//!    - If it carries `leaf_data` → CRDT merge the entity
//!    - If `has_children` → add to next_level_ids
//!      (a collection container is both: its own row is the only source of its
//!      `own_hash`, and its children still have to be walked)
//! 4. Request level 1 with parent_ids = those nodes
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
//!     LevelWiseConfig { remote_root_hash, max_depth: 2, context_client: Some(client) },
//! ).await?;
//!
//! // Responder side (manager extracts first request data)
//! let first_request = LevelWiseFirstRequest { level: 0, parent_ids: None, context_client: Some(client) };
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
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::sync::{
    compare_level_nodes, create_runtime_env, EntityDeletion, InitPayload, InitProof, LevelNode,
    MessagePayload, StreamMessage, SyncProtocolExecutor, SyncTransport, TreeLeafData,
    MAX_LEVELWISE_DEPTH, MAX_NODES_PER_LEVEL, MAX_PARENTS_PER_REQUEST, MAX_REQUESTS_PER_SESSION,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::collections::is_app_root_entry;
use calimero_storage::env::with_runtime_env;
use calimero_storage::index::Index;
use calimero_storage::store::MainStorage;
use calimero_store::Store;
use eyre::{bail, Result};
use tracing::{debug, info, trace, warn};

use crate::sync::hash_comparison_protocol::{entity_wire_row, local_entity_wire_row};
use crate::sync::helpers::{
    apply_leaf_with_crdt_merge, apply_leaf_with_crdt_merge_gated, apply_under_context_lock,
    classify_leaf, generate_nonce, get_local_root_hash_for_context,
    handle_entity_delete_push_locked, handle_entity_push_locked, is_leaf_currently_authorized,
    push_entities, LeafDisposition, LeafOutcome, MAX_ENTITIES_PER_PUSH,
};

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
    /// Client used to acquire the per-context execution lock around the
    /// initiator's host-side leaf/tombstone applies (split-brain guard). `None`
    /// in the single-threaded sync-sim harness.
    pub context_client: Option<ContextClient>,
    /// Remote peer's attributable member identity — gates authorless leaves on
    /// the peer's current membership. See `HashComparisonConfig::session_peer`.
    pub session_peer: Option<PublicKey>,
    /// Transport-binding proof of possession for `identity`, attached to every
    /// state-read `Init` this initiator sends. See `HashComparisonConfig::init_pop`
    /// and [`InitProof`].
    pub init_pop: Option<InitProof>,
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
    /// Client used to acquire the per-context execution lock around the
    /// responder's host-side tombstone applies (split-brain guard). `None` in
    /// the single-threaded sync-sim harness.
    pub context_client: Option<ContextClient>,
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
    /// Whether parent_ids were truncated due to `MAX_PARENTS_PER_REQUEST`.
    ///
    /// If true, the sync may be incomplete and a follow-up sync might be needed.
    pub truncation_occurred: bool,
    /// Root-state byte blobs the level-by-level walk encountered on
    /// remote leaves that the host can't merge itself. Same shape +
    /// rationale as `HashComparisonStats::deferred_root_merges`; the
    /// caller (`ProtocolSelector`) dispatches them through
    /// `ContextClient::merge_root_state` after the sync completes.
    /// Each entry is `(entity_id_bytes, incoming_bytes, incoming_hlc_ts)`.
    pub deferred_root_merges: Vec<([u8; 32], Vec<u8>, u64)>,

    /// Custom-typed ENTRIES deferred for WASM dispatch; same rationale as
    /// `HashComparisonStats::deferred_custom_merges`. Applying one here would
    /// fall through to LWW and contradict the in-WASM delta path.
    pub deferred_custom_merges: Vec<(
        [u8; 32],
        calimero_primitives::crdt::CustomTypeId,
        Vec<u8>,
        u64,
    )>,
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
            config.context_client.as_ref(),
            config.session_peer,
            config.init_pop,
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
            first_request.context_client,
        )
        .await
    }
}

// =============================================================================
// Initiator Implementation
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn run_initiator_impl<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    identity: PublicKey,
    remote_root_hash: [u8; 32],
    max_depth: u32,
    context_client: Option<&ContextClient>,
    session_peer: Option<PublicKey>,
    init_pop: Option<InitProof>,
) -> Result<LevelWiseStats> {
    info!(
        %context_id,
        max_depth,
        remote_root = %hex::encode(&remote_root_hash[..8]),
        "Starting LevelWise sync (initiator)"
    );

    let mut stats = LevelWiseStats::default();

    // Set up storage bridge
    let account = calimero_governance_store::account_for_context(store, &context_id)?;
    let runtime_env = create_runtime_env(store, context_id, identity, account);

    // The sender's loaded-reader schema, stamped onto every row we push back.
    let schema_bytecode_id =
        calimero_context::hlc_fence::loaded_reader_bytecode_id(store, &context_id)
            .ok()
            .flatten();

    // Container rows to hand back to the peer, and the entities already queued,
    // which bounds the repair to one row per entity per session.
    let mut pending_row_pushes: Vec<TreeLeafData> = Vec::new();
    let mut pushed_rows: HashSet<[u8; 32]> = HashSet::new();

    // Track which parent IDs to query at next level
    // Start with None = request all nodes at level 0 (root's children)
    let mut current_parent_ids: Option<Vec<[u8; 32]>> = None;
    let clamped_max_depth = max_depth.min(MAX_LEVELWISE_DEPTH as u32);

    // Deletions to propagate: entries the remote still holds that we have
    // locally tombstoned (cleared). LevelWise is pull-only, so without this
    // the deletion never reaches the peer and the clear can't converge via
    // this protocol (same bug HashComparison had). Collected across levels and
    // flushed once after the walk. Mirrors HashComparison's remote-only path.
    let mut pending_deletions: Vec<EntityDeletion> = Vec::new();

    for level in 0..clamped_max_depth {
        // Build request for this level
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            pop: init_pop,
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

        let (resp_level, mut nodes, has_more_levels, remote_deleted) = match payload {
            MessagePayload::LevelWiseResponse {
                level: resp_level,
                nodes,
                has_more_levels,
                deleted_children,
            } => (resp_level, nodes, has_more_levels, deleted_children),
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
            deleted_received = remote_deleted.len(),
            has_more_levels,
            "Received level response"
        );

        // Apply any tombstones the responder advertised for this level FIRST —
        // before the empty-nodes early-out below. A cleared collection returns
        // zero live nodes but a non-empty `deleted_children`; applying here
        // (authenticated DeleteRef, delete-wins) is what converges a clear when
        // the holder initiates. Safe no-op when we already deleted or lose LWW.
        if !remote_deleted.is_empty() {
            let applied =
                apply_under_context_lock(context_client, context_id, &runtime_env, || {
                    crate::sync::hash_comparison_protocol::apply_remote_tombstones(
                        context_client.map(ContextClient::datastore),
                        context_id,
                        &remote_deleted,
                    )
                })
                .await;
            stats.entities_merged += applied;
            debug!(
                %context_id,
                level,
                advertised = remote_deleted.len(),
                applied,
                "Applied remote tombstones from level response"
            );
        }

        if nodes.is_empty() {
            debug!(%context_id, level, "No nodes at this level, sync complete");
            break;
        }

        // Get local hashes for comparison
        let local_hashes = with_runtime_env(runtime_env.clone(), || {
            get_local_hashes_at_level(context_id, current_parent_ids.as_deref())
        })?;

        // Compare local vs remote - pass nodes directly to avoid wrap-unwrap
        let compare_result = compare_level_nodes(&local_hashes, &nodes);

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

        // Nodes present on both sides with different hashes, as a set: only
        // those can carry a container-row disagreement worth pushing back.
        let differing: HashSet<[u8; 32]> = compare_result.differing.iter().copied().collect();

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

            // Tombstone propagation: if we hold a local tombstone for a node the
            // remote still has (it surfaces here because it's absent from our
            // live tree), propagate our deletion instead of pulling the remote's
            // live copy. The peer applies it via the authenticated DeleteRef
            // path. Mirrors HashComparison's remote-only handling.
            let tombstone =
                with_runtime_env(
                    runtime_env.clone(),
                    || match Index::<MainStorage>::get_index(calimero_storage::address::Id::new(
                        node_id,
                    )) {
                        Ok(Some(idx)) => idx.deleted_at.map(|d| (d, idx.metadata.clone())),
                        _ => None,
                    },
                );
            if let Some((deleted_at, metadata)) = tombstone {
                pending_deletions.push(EntityDeletion {
                    id: node_id,
                    deleted_at,
                    metadata,
                });
                continue;
            }

            // Both sides hold this node and the hashes disagree, so the peer may
            // be the one holding a container index with no bytes - nothing it
            // can request would repair that. Once per entity per session.
            if node.has_children && differing.contains(&node_id) && pushed_rows.insert(node_id) {
                if let Some(row) = with_runtime_env(runtime_env.clone(), || {
                    local_entity_wire_row(&node_id, schema_bytecode_id)
                }) {
                    pending_row_pushes.push(row);
                }
            }

            // A container carries a row AND children: apply the row, then still
            // descend. Reading "has a row" as "is a leaf" is what made the two
            // mutually exclusive and left every container's bytes behind.
            if let Some(leaf_data) = node.leaf_data.as_ref() {
                merge_remote_row(
                    store,
                    context_id,
                    &runtime_env,
                    context_client,
                    session_peer,
                    leaf_data,
                    &mut stats,
                )
                .await?;
            }

            if node.has_children && added_parents.insert(node.id) {
                next_level_parents.push(node.id);
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
            // Track that truncation occurred - sync may be incomplete
            stats.truncation_occurred = true;
        }

        current_parent_ids = Some(next_level_parents);
    }

    // Flush the container-row repairs collected during the walk, in batches so
    // the session's request budget stays bounded.
    if !pending_row_pushes.is_empty() {
        let (applied, batches) =
            push_entities(transport, context_id, identity, &pending_row_pushes).await?;
        stats.requests_sent += batches;
        debug!(
            %context_id,
            rows = pending_row_pushes.len(),
            applied,
            "Flushed container-row repairs to the peer"
        );
    }

    // Flush deletion propagation (clear convergence). Entries we cleared but the
    // peer still holds are pushed as authenticated tombstones so the peer
    // applies delete-wins, instead of us silently re-pulling them.
    if !pending_deletions.is_empty() {
        for chunk in pending_deletions.chunks(MAX_ENTITIES_PER_PUSH) {
            let msg = StreamMessage::Init {
                context_id,
                party_id: identity,
                payload: InitPayload::EntityDeletePush {
                    context_id,
                    deletions: chunk.to_vec(),
                },
                next_nonce: generate_nonce(),
                // Write (tombstone) — authorized per-action on apply, not by the
                // sender's `party_id`; no read-gating proof needed.
                pop: None,
            };
            transport.send(&msg).await?;

            let ack = transport.recv().await?.ok_or_else(|| {
                eyre::eyre!("stream closed while waiting for EntityDeletePushAck")
            })?;
            match ack {
                StreamMessage::Message {
                    payload: MessagePayload::EntityDeletePushAck { applied_count },
                    ..
                } => {
                    debug!(%context_id, applied_count, "LevelWise: peer applied pushed tombstones");
                }
                _ => bail!("Unexpected response to EntityDeletePush"),
            }
        }
    }

    // C1b: re-query the peer's CURRENT root + scope_root before closing — the
    // #2407 comment below noted LevelWise "can't distinguish a real divergence bug
    // from legitimate drift without a second handshake round-trip"; this IS that
    // round-trip (parity with HashComparison's #2607 end-of-session read). Falls
    // back to the stale handshake root only if the peer doesn't answer (older
    // peer).
    let (peer_current_root, peer_scope_root) =
        match super::hash_comparison_protocol::query_peer_current_root(
            transport, context_id, identity, init_pop,
        )
        .await
        {
            Ok(Some((root, scope_root))) => (root, scope_root),
            // Older peer that doesn't answer the re-query — expected, quiet.
            Ok(None) => (remote_root_hash, None),
            // Transport fault on the re-query: distinct from the older-peer case, so
            // log it (a persistent fault degrades the verdict to the stale handshake
            // root and would otherwise masquerade as a quiet version mismatch).
            Err(e) => {
                debug!(
                    %context_id, %e,
                    "LevelWise: end-of-session peer re-query failed; \
                     falling back to the handshake root for the convergence check"
                );
                (remote_root_hash, None)
            }
        };

    // Close the transport to signal completion
    transport.close().await?;

    // Post-sync convergence check. `scope_root` is the AUTHORITATIVE verdict (C1b),
    // parity with HashComparison: it folds the governance projection's ACL +
    // membership onto the entity root, so LevelWise can't declare converged while
    // authorization disagrees. When either side can't fold the scope (cold
    // projection / non-group context, `None`), fall back to the entity-root compare
    // — now against the peer's RE-QUERIED current root rather than the stale
    // handshake root, so even the fallback is sharper than the prior #2407 check.
    let local_root_hash = with_runtime_env(runtime_env.clone(), || {
        get_local_root_hash_for_context(context_id)
    })?;

    let local_scope_root = super::helpers::local_scope_root(store, &context_id, local_root_hash);
    let verdict = super::helpers::scope_verdict(
        local_scope_root,
        peer_scope_root,
        local_root_hash,
        peer_current_root,
    );
    stats.root_hash_verified = verdict.converged();

    if !stats.root_hash_verified {
        // Entities agree but scope_root differs ⇒ pure ACL/governance divergence
        // (the case the entity root hides); observability only — the corrective
        // governance pull is centralised post-sync in the manager (P6.S3), covering
        // all data backends, not just HC / LevelWise.
        if let super::helpers::ScopeVerdict::GovDiverged(local_scope_root, peer_scope_root) =
            verdict
        {
            warn!(
                marker = "scope_root_governance_divergence",
                %context_id,
                entity_root = %hex::encode(&local_root_hash[..8]),
                local_scope_root = %hex::encode(&local_scope_root[..8]),
                peer_scope_root = %hex::encode(&peer_scope_root[..8]),
                "entity roots agree but scope_root differs — ACL/membership divergence; \
                 pulling governance from the peer to propagate the rotation"
            );
        } else {
            warn!(
                %context_id,
                local_hash = %hex::encode(&local_root_hash[..8]),
                peer_hash = %hex::encode(&peer_current_root[..8]),
                levels_synced = stats.levels_synced,
                nodes_compared = stats.nodes_compared,
                entities_merged = stats.entities_merged,
                "LevelWise sync did not converge with the peer's re-queried root. \
                 Persistent occurrences across many interval-sync ticks indicate a \
                 real merge convergence bug."
            );
        }
    } else {
        debug!(
            %context_id,
            root_hash = %hex::encode(&local_root_hash[..8]),
            "Root hash verified after sync"
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

/// CRDT-merge one entity row the peer sent, or record it for the caller to
/// dispatch when the host cannot merge it itself (app-typed root and custom
/// entities). A container's row arrives through here as well as a leaf's.
async fn merge_remote_row(
    store: &Store,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_client: Option<&ContextClient>,
    session_peer: Option<PublicKey>,
    leaf_data: &TreeLeafData,
    stats: &mut LevelWiseStats,
) -> Result<()> {
    trace!(%context_id, key = %hex::encode(leaf_data.key), "Merging entity row");

    // Same per-leaf membership gate as the HashComparison initiator (LevelWise
    // walks the same merge path); drops a revoked author's / revoked peer's
    // leaves.
    if !is_leaf_currently_authorized(store, &context_id, leaf_data, session_peer) {
        warn!(
            %context_id,
            key = %hex::encode(leaf_data.key),
            "LevelWise merge skipped: claimed author is not currently authorized for this context"
        );
        return Ok(());
    }

    // Defer root entities with a real `crdt_type` for WASM dispatch; opaque
    // root entities (synthetic `Opaque` LWW marker) fall through to
    // `apply_leaf_with_crdt_merge` which LWW-writes them directly (no
    // Mergeable to dispatch).
    let entity_id = Id::new(leaf_data.key);
    match classify_leaf(entity_id, &leaf_data.metadata.crdt_type) {
        LeafDisposition::DeferRoot => {
            stats.deferred_root_merges.push((
                leaf_data.key,
                leaf_data.value.clone(),
                leaf_data.metadata.hlc_timestamp,
            ));
            return Ok(());
        }
        LeafDisposition::DeferCustom(type_id) => {
            stats.deferred_custom_merges.push((
                leaf_data.key,
                type_id,
                leaf_data.value.clone(),
                leaf_data.metadata.hlc_timestamp,
            ));
            return Ok(());
        }
        LeafDisposition::Apply => {}
    }

    // PR-6b Task 6b.7: gate on the loaded reader so a future-schema leaf is
    // declined+buffered rather than LWW-stored as unreadable bytes.
    let loaded_bytecode_id =
        calimero_context::hlc_fence::loaded_reader_bytecode_id(store, &context_id)
            .ok()
            .flatten();
    let outcome = apply_under_context_lock(context_client, context_id, runtime_env, || {
        match loaded_bytecode_id {
            Some(loaded) => apply_leaf_with_crdt_merge_gated(store, context_id, leaf_data, loaded),
            None => {
                apply_leaf_with_crdt_merge(context_id, leaf_data).map(|()| LeafOutcome::Applied)
            }
        }
    })
    .await?;
    match outcome {
        LeafOutcome::Applied => stats.entities_merged += 1,
        LeafOutcome::Buffered => {
            // Declined: buffered, not applied. A later drain replays it.
        }
    }

    Ok(())
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
    context_client: Option<ContextClient>,
) -> Result<()> {
    info!(%context_id, "Starting LevelWise sync (responder)");

    // Defense in depth: validate first request parameters
    // (The manager should have validated these, but we check again)
    if (first_level as usize) > MAX_LEVELWISE_DEPTH {
        bail!(
            "First request level {} exceeds maximum {}",
            first_level,
            MAX_LEVELWISE_DEPTH
        );
    }
    if let Some(ref parents) = first_parent_ids {
        if parents.len() > MAX_PARENTS_PER_REQUEST {
            bail!(
                "First request parent_ids count {} exceeds maximum {}",
                parents.len(),
                MAX_PARENTS_PER_REQUEST
            );
        }
    }

    // Set up storage bridge
    let account = calimero_governance_store::account_for_context(store, &context_id)?;
    let runtime_env = create_runtime_env(store, context_id, identity, account);

    // The sender's loaded-reader schema, stamped onto every row we emit so a
    // peer on an older reader can decline+buffer a future-schema one.
    let schema_bytecode_id =
        calimero_context::hlc_fence::loaded_reader_bytecode_id(store, &context_id)
            .ok()
            .flatten();

    let mut sequence_id = 0u64;

    // Handle the first request (already parsed by the manager)
    let (nodes, has_more_levels, deleted_children) = handle_levelwise_request(
        context_id,
        first_level,
        first_parent_ids,
        &runtime_env,
        schema_bytecode_id,
    )?;

    debug!(
        %context_id,
        level = first_level,
        nodes_found = nodes.len(),
        deleted = deleted_children.len(),
        has_more_levels,
        "Responding with first level nodes"
    );

    let response = StreamMessage::Message {
        sequence_id,
        payload: MessagePayload::LevelWiseResponse {
            level: first_level,
            nodes,
            has_more_levels,
            deleted_children,
        },
        next_nonce: generate_nonce(),
    };
    transport.send(&response).await?;
    sequence_id += 1;

    // Handle subsequent requests in a loop
    run_responder_loop(
        transport,
        store,
        context_id,
        &runtime_env,
        sequence_id,
        1,
        context_client.as_ref(),
        schema_bytecode_id,
    )
    .await
}

/// Handle a single LevelWise request and return the response data.
fn handle_levelwise_request(
    context_id: ContextId,
    level: u32,
    parent_ids: Option<Vec<[u8; 32]>>,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    schema_bytecode_id: Option<[u8; 32]>,
) -> Result<(Vec<LevelNode>, bool, Vec<EntityDeletion>)> {
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
        return Ok((vec![], false, vec![]));
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
        get_nodes_at_level(
            context_id,
            level as usize,
            truncated_parent_ids.as_deref(),
            schema_bytecode_id,
        )
    })
}

/// Internal loop to handle subsequent LevelWise requests.
#[allow(clippy::too_many_arguments)]
async fn run_responder_loop<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    mut sequence_id: u64,
    initial_requests_handled: u64,
    context_client: Option<&ContextClient>,
    schema_bytecode_id: Option<[u8; 32]>,
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

        match payload {
            InitPayload::LevelWiseRequest {
                level, parent_ids, ..
            } => {
                let (nodes, has_more_levels, deleted_children) = handle_levelwise_request(
                    context_id,
                    level,
                    parent_ids,
                    runtime_env,
                    schema_bytecode_id,
                )?;

                debug!(
                    %context_id,
                    level,
                    nodes_found = nodes.len(),
                    deleted = deleted_children.len(),
                    has_more_levels,
                    "Responding with level nodes"
                );

                let response = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::LevelWiseResponse {
                        level,
                        nodes,
                        has_more_levels,
                        deleted_children,
                    },
                    next_nonce: generate_nonce(),
                };

                transport.send(&response).await?;
                sequence_id += 1;
                requests_handled += 1;
            }

            // Container-row repair. LevelWise is otherwise initiator-pull, so a
            // responder that materialised a container from a descendant's
            // ancestor chain holds an index row with no bytes and nothing it
            // can ask for would fix it; the initiator hands its own row over.
            // Same apply path as HashComparison's EntityPush.
            InitPayload::EntityPush { entities, .. } => {
                let total = entities.len();
                trace!(%context_id, total, "Handling EntityPush from initiator");

                let outcome = handle_entity_push_locked(
                    context_client,
                    store,
                    runtime_env,
                    context_id,
                    &entities,
                    None,
                )
                .await;

                // This responder has no `ContextClient` in the trait signature's
                // reach for app-typed root state, so it can't dispatch deferred
                // root merges; the initiator's own walk picks that divergence up
                // on the next round. Same gap, and same reasoning, as the
                // HashComparison protocol responder.
                if !outcome.deferred_root_merges.is_empty() {
                    warn!(
                        %context_id,
                        deferred = outcome.deferred_root_merges.len(),
                        "LevelWise EntityPush: dropped root-entity deferred merges"
                    );
                }

                let response = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::EntityPushAck {
                        applied_count: outcome.applied,
                    },
                    next_nonce: generate_nonce(),
                };
                transport.send(&response).await?;
                sequence_id += 1;
                requests_handled += 1;

                info!(
                    %context_id,
                    applied = outcome.applied,
                    total,
                    "Applied pushed entities via CRDT merge"
                );
            }

            // Tombstone propagation (clear convergence) — same mechanism as
            // HashComparison. The cleared initiator pushes authenticated
            // deletions for entries this responder still holds; apply them via
            // the DeleteRef path (delete-wins by HLC; sig/nonce verified for
            // User/Shared, safe no-op on loss/auth-fail).
            InitPayload::EntityDeletePush { deletions, .. } => {
                let total = deletions.len();
                trace!(%context_id, total, "Handling EntityDeletePush from initiator");

                // Apply under the per-context execution lock so the tombstone
                // writes can't interleave with a concurrent delta merge.
                let applied = handle_entity_delete_push_locked(
                    context_client,
                    context_id,
                    runtime_env,
                    &deletions,
                )
                .await;

                let response = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::EntityDeletePushAck {
                        applied_count: applied,
                    },
                    next_nonce: generate_nonce(),
                };
                transport.send(&response).await?;
                sequence_id += 1;
                requests_handled += 1;

                info!(%context_id, applied, total, "Applied pushed tombstones (delete-wins)");
            }

            // C1b: end-of-session convergence re-read, parity with HashComparison
            // (the #2607 path). Re-read our root NOW — after every leaf/tombstone
            // applied this session — and fold the governance projection onto it so
            // the initiator's verdict catches a hash-neutral ACL/membership
            // divergence the bare entity root hides. `scope_root` is `None` on a
            // non-group / cold projection ⇒ the initiator falls back to the entity
            // compare.
            InitPayload::DagHeadsRequest { .. } => {
                let current_root = with_runtime_env(runtime_env.clone(), || {
                    get_local_root_hash_for_context(context_id)
                })
                .unwrap_or([0u8; 32]);

                let scope_root = context_client
                    .map(|cc| cc.datastore_handle().into_inner())
                    .and_then(|store| {
                        super::helpers::local_scope_root(&store, &context_id, current_root)
                    })
                    .map(calimero_primitives::hash::Hash::from);

                let response = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::DagHeadsResponse {
                        dag_heads: Vec::new(),
                        root_hash: calimero_primitives::hash::Hash::from(current_root),
                        scope_root,
                    },
                    next_nonce: generate_nonce(),
                };
                transport.send(&response).await?;
                sequence_id += 1;
                requests_handled += 1;
            }

            _ => {
                debug!(%context_id, "Received non-LevelWiseRequest, ending responder");
                break;
            }
        }
    }

    info!(%context_id, requests_handled, "LevelWise responder complete");
    Ok(())
}

// =============================================================================
// Storage Helpers
// =============================================================================

/// Get local node hashes at a level for comparison.
///
/// Returns a map of node_id -> hash for all nodes at the specified level.
fn get_local_hashes_at_level(
    context_id: ContextId,
    parent_ids: Option<&[[u8; 32]]>,
) -> Result<HashMap<[u8; 32], [u8; 32]>> {
    let mut hashes = HashMap::new();

    let root_id = Id::new(*context_id.as_ref());

    // Existence only. The children come from the trie below, so the decoded row
    // itself is not wanted — binding it and then reaching for
    // `get_children_of` is what made this a doubled read.
    match Index::<MainStorage>::get_index(root_id) {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(hashes), // Empty tree
        Err(e) => {
            warn!(%context_id, error = %e, "Failed to get root index");
            return Ok(hashes);
        }
    }

    match parent_ids {
        None => {
            // Level 0: get direct children of root
            {
                // `root_id`, not `Id::root()`: the latter derives the context
                // from the `RUNTIME_ENV` thread-local, and a divergence there
                // yields silently empty level-0 hashes rather than an error.
                //
                // The trie directly, not `get_children_of`: that re-reads and
                // re-decodes the index row this function just proved decodable
                // — a doubled read per parent per level, on a protocol chosen
                // for wide trees — and its only failure here is that re-read,
                // so `unwrap_or_default` would turn a corrupt row into "no
                // children" instead of surfacing it.
                for child in ChildTrie::<MainStorage>::new(root_id).children() {
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
                if let Ok(Some(_)) = Index::<MainStorage>::get_index(parent_storage_id) {
                    {
                        // Same as level 0: existence is already established, so
                        // walk the trie rather than re-reading the row.
                        for child in ChildTrie::<MainStorage>::new(parent_storage_id).children() {
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
    schema_bytecode_id: Option<[u8; 32]>,
) -> Result<(Vec<LevelNode>, bool, Vec<EntityDeletion>)> {
    let mut nodes = Vec::new();
    let mut deleted_children = Vec::new();
    let mut has_more_levels = false;

    let root_id = Id::new(*context_id.as_ref());

    // Verify root exists before proceeding
    match Index::<MainStorage>::get_index(root_id) {
        Ok(Some(_)) => {}                                        // Root exists, continue
        Ok(None) => return Ok((nodes, false, deleted_children)), // Empty tree
        Err(e) => {
            warn!(%context_id, error = %e, "Failed to get root index");
            return Ok((nodes, false, deleted_children));
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
            return Ok((nodes, false, deleted_children));
        }
        Some(ids) => ids.iter().map(|id| Id::new(*id)).collect(),
    };

    for parent_id in parents_to_query {
        let parent_index = match Index::<MainStorage>::get_index(parent_id) {
            Ok(Some(idx)) => idx,
            Ok(None) => continue,
            Err(_) => continue,
        };

        // Tombstones for children this parent deleted ride alongside the live
        // node list so a holder (the initiator) learns of the deletion even
        // when the parent now has zero live children (the cleared-collection
        // case, where the node list would otherwise be empty).
        deleted_children.extend(
            crate::sync::hash_comparison_protocol::collect_deleted_children_wire(&parent_index),
        );

        let children = Index::<MainStorage>::get_children_of(parent_id).unwrap_or_default();
        if children.is_empty() {
            continue;
        }

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
            // `has_children`, not `get_children_of(..).is_empty()`: the latter
            // walks every node and bucket row of that child's trie, allocates
            // every ChildInfo and sorts them — per child, per level, on the sync
            // hot path. For the ~1,600-child collection this work exists to fix,
            // that is the linear read put back, just on the read side.
            let has_children = Index::<MainStorage>::has_children(child.id()).unwrap_or(false);
            has_more_levels |= has_children;

            // Determine parent_id for this node (None for level 0)
            let parent_id_bytes = if level == 0 {
                None
            } else {
                Some(*parent_id.as_bytes())
            };

            // An internal node carries its own row too: a container's bytes are
            // the only source of its `own_hash`. Excluded exactly as hash
            // comparison excludes it, the app root being merged by the app.
            let wire_row = if has_children && is_app_root_entry(child_storage_id) {
                None
            } else {
                entity_wire_row(child_storage_id, &child_index, schema_bytecode_id)
            };

            match (wire_row, has_children) {
                (Some(leaf_data), false) => {
                    nodes.push(LevelNode::leaf(
                        child_id,
                        child_hash,
                        parent_id_bytes,
                        leaf_data,
                    ));
                }
                (Some(leaf_data), true) => {
                    nodes.push(LevelNode::container(
                        child_id,
                        child_hash,
                        parent_id_bytes,
                        leaf_data,
                    ));
                }
                (None, true) => {
                    nodes.push(LevelNode::internal(child_id, child_hash, parent_id_bytes));
                }
                (None, false) => {
                    // Childless and row-less: nothing to hand over, and the
                    // peer's `is_valid` would reject it anyway.
                    debug!(
                        %context_id,
                        child_id = %hex::encode(&child_id[..8]),
                        "Skipping leaf node with no raw data"
                    );
                    continue;
                }
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

    Ok((nodes, has_more_levels, deleted_children))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A container is handed over as a node that carries BOTH its own row and
    /// `has_children`, which is the combination the pre-fix wire could not
    /// express; leaves keep their row and the app root is still left out.
    #[test]
    fn a_containers_own_row_rides_along_with_has_children() {
        use std::sync::Arc;

        use calimero_primitives::context::ContextId;
        use calimero_storage::action::Action;
        use calimero_storage::entities::{ChildInfo, Metadata};
        use calimero_storage::interface::{ApplyContext, Interface};
        use calimero_store::db::InMemoryDB;

        let context_id = ContextId::from([0xCA; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
        );

        let root_id = Id::new(*context_id.as_ref());
        let container_id = Id::new([0x5C; 32]);
        let child_id = Id::new([0x5D; 32]);

        with_runtime_env(runtime_env, || {
            let add_under = |parent: Id, id: Id, data: Vec<u8>| {
                let parent_hash = Index::<MainStorage>::get_hashes_for(parent)
                    .ok()
                    .flatten()
                    .map_or([0; 32], |(full, _)| full);
                let parent_meta = Index::<MainStorage>::get_index(parent)
                    .ok()
                    .flatten()
                    .map(|idx| idx.metadata.clone())
                    .unwrap_or_default();
                Interface::<MainStorage>::apply_action(
                    Action::Add {
                        id,
                        data,
                        ancestors: vec![ChildInfo::new(parent, parent_hash, parent_meta)],
                        metadata: Metadata::new(100, 100),
                    },
                    &ApplyContext::empty(),
                )
                .expect("add entity");
            };

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
            add_under(root_id, container_id, container_id.as_bytes().to_vec());
            add_under(container_id, child_id, b"entry".to_vec());

            let (level_zero, has_more_levels, _deleted) =
                get_nodes_at_level(context_id, 0, None, None).expect("level 0");
            assert!(
                has_more_levels,
                "the container's children are a level below"
            );

            let container = level_zero
                .iter()
                .find(|node| node.id == *container_id.as_bytes())
                .expect("the container must appear at level 0");
            assert!(
                container.has_children,
                "a container must still be descended into"
            );
            assert_eq!(
                container.leaf_data.as_ref().map(|row| row.value.clone()),
                Some(container_id.as_bytes().to_vec()),
                "the container's own row is the only source of its own_hash"
            );

            let (level_one, _has_more, _deleted) =
                get_nodes_at_level(context_id, 1, Some(&[*container_id.as_bytes()]), None)
                    .expect("level 1");
            let child = level_one
                .iter()
                .find(|node| node.id == *child_id.as_bytes())
                .expect("the child must appear at level 1");
            assert!(!child.has_children, "the child is a leaf");
            assert!(child.leaf_data.is_some(), "a leaf still carries its row");
        });
    }

    #[test]
    fn test_config_creation() {
        let config = LevelWiseConfig {
            remote_root_hash: [1u8; 32],
            max_depth: 2,
            context_client: None,
            session_peer: None,
            init_pop: None,
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
        let stats = LevelWiseStats {
            levels_synced: 2,
            nodes_compared: 100,
            entities_merged: 25,
            nodes_skipped: 75,
            max_nodes_per_level: 50,
            requests_sent: 3,
            root_hash_verified: true,
            ..Default::default()
        };

        assert_eq!(stats.levels_synced, 2);
        assert_eq!(stats.nodes_compared, 100);
        assert_eq!(stats.entities_merged, 25);
        assert_eq!(stats.nodes_skipped, 75);
        assert_eq!(stats.max_nodes_per_level, 50);
        assert_eq!(stats.requests_sent, 3);
        assert!(stats.root_hash_verified);
    }
}
