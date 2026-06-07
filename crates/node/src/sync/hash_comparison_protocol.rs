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
//!     HashComparisonConfig { remote_root_hash, context_client: Some(client) },
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
    apply_leaf_with_crdt_merge, apply_leaf_with_crdt_merge_gated, apply_under_context_lock,
    generate_nonce, get_local_root_hash_for_context, handle_entity_push,
    is_leaf_currently_authorized, LeafOutcome, MAX_ENTITIES_PER_PUSH,
};
use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::sync::{
    compare_tree_nodes, create_runtime_env, EntityDeletion, InitPayload, LeafMetadata,
    MessagePayload, StreamMessage, SyncProtocolExecutor, SyncTransport, TreeCompareResult,
    TreeLeafData, TreeNode, TreeNodeResponse, MAX_LEAF_VALUE_SIZE, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::StorageType;
use calimero_storage::env::with_runtime_env;
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::rotation_log::{self, RotationLog};
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
pub(crate) const OPAQUE_LEAF_CRDT_TYPE_NAME: &str = "Opaque";

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
    /// Client used to acquire the per-context execution lock so the initiator's
    /// host-side leaf/tombstone applies are mutually exclusive with a concurrent
    /// delta merge in the executor. `None` in the single-threaded sync-sim
    /// harness, where no executor runs alongside the protocol.
    pub context_client: Option<ContextClient>,
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
    /// Root-state byte blobs the DFS encountered on remote leaves
    /// that the host can't merge by itself (separate-address-space
    /// merge registry — see [`crate::sync::helpers::apply_leaf_with_crdt_merge`]).
    /// Each entry is `(entity_id_bytes, incoming_bytes, incoming_hlc_ts)`.
    /// The caller (`ProtocolSelector`) dispatches each one through
    /// `ContextClient::merge_root_state` after the sync completes,
    /// closing the loop on root-entity divergence that HC would
    /// otherwise silently drop. Storing the entity id lets the caller
    /// distinguish `ROOT_ID` from the `Root<T>` entry (both treated
    /// as root by `is_app_root_entry`, both possible in HC leaves);
    /// the timestamp is the leaf's wire-carried `hlc_timestamp` so
    /// the dispatch uses the actual remote write time instead of a
    /// synthetic value.
    pub deferred_root_merges: Vec<([u8; 32], Vec<u8>, u64)>,
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
            config.context_client.as_ref(),
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
    context_client: Option<&ContextClient>,
) -> Result<HashComparisonStats> {
    info!(%context_id, "Starting HashComparison sync (initiator)");

    let mut stats = HashComparisonStats::default();

    // Set up storage bridge
    let runtime_env = create_runtime_env(store, context_id, identity);

    // PR-6b Task 6b.7: the sender's loaded-reader schema, stamped onto every
    // leaf we emit so a peer on an older reader can decline+buffer a
    // future-schema leaf. `None` when unresolvable (no group / missing meta).
    let schema_app_key = calimero_context::hlc_fence::loaded_reader_app_key(store, &context_id)
        .ok()
        .flatten();

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

    // Deletions the initiator must propagate to the peer: children the peer
    // still holds that we have locally tombstoned (cleared). HashComparison's
    // child comparison is add-wins, so without this the peer's live copy would
    // simply be re-pulled and the deletion would never converge (the clear
    // split-brain). Collected during the DFS and flushed once after the walk.
    let mut pending_deletions: Vec<EntityDeletion> = Vec::new();

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

                    // Authorization gate, parity with `handle_entity_push`.
                    // Without this, the initiator's per-leaf merge in the
                    // DFS would re-import entities whose claimed author has
                    // been removed from the context's group — the same back
                    // door batched EntityPush had before the helper-level
                    // filter landed. Skipping silently here is fine because
                    // the leaf will simply remain "missing locally," and
                    // `root_hash_verified` will report `false` so the
                    // session is treated as a partial merge rather than a
                    // successful convergence.
                    if !is_leaf_currently_authorized(store, &context_id, leaf_data) {
                        warn!(
                            %context_id,
                            key = %hex::encode(leaf_data.key),
                            "HC merge skipped: claimed author is not currently authorized for this context"
                        );
                        continue;
                    }

                    // Root entity leaves can't be merged on the host
                    // (the host's `merge_root_state` consults a registry
                    // that's only populated inside WASM). Hand them off
                    // to the caller, which dispatches each through
                    // `ContextClient::merge_root_state` after the sync
                    // session completes. `apply_leaf_with_crdt_merge`
                    // also short-circuits root entities — we check here
                    // too so we can record the incoming bytes (the helper
                    // is sync and inside `with_runtime_env`, so it can't
                    // call into the runtime to do the merge itself).
                    // Defer root entities with a real `crdt_type` for
                    // WASM dispatch; opaque root entities (synthetic
                    // `Opaque` LWW marker) fall through to
                    // `apply_leaf_with_crdt_merge` which LWW-writes
                    // them directly (no Mergeable to dispatch).
                    let entity_id = calimero_storage::address::Id::new(leaf_data.key);
                    let is_opaque = matches!(
                        &leaf_data.metadata.crdt_type,
                        calimero_primitives::crdt::CrdtType::LwwRegister { inner_type }
                            if inner_type == OPAQUE_LEAF_CRDT_TYPE_NAME
                    );
                    if calimero_storage::collections::is_app_root_entry(entity_id) && !is_opaque {
                        stats.deferred_root_merges.push((
                            leaf_data.key,
                            leaf_data.value.clone(),
                            leaf_data.metadata.hlc_timestamp,
                        ));
                        continue;
                    }

                    // PR-6b Task 6b.7: gate on the loaded reader. The HC repair
                    // path bypasses the gossip state-delta fence, so a leaf
                    // authored under a newer schema than this node's loaded
                    // reader must be declined+buffered (into the absorb buffer)
                    // rather than LWW-stored as unreadable bytes — the
                    // v1-binary-fed-v2-bytes corruption hazard.
                    //
                    // Keep the FULL `Result` (no `.ok().flatten()`): a STORE
                    // ERROR must not silently disable the gate. `apply_hc_leaf_gated`
                    // distinguishes `Err` (fail closed: skip the leaf, re-pushed
                    // next sync) from `Ok(None)` (legitimately no group ⇒ apply
                    // ungated as today) — see `apply_entity_push_batch`.
                    let loaded_app_key =
                        calimero_context::hlc_fence::loaded_reader_app_key(store, &context_id);

                    // Under the per-context execution lock: this leaf merge is a
                    // read-modify-write up to the root and must not interleave
                    // with a concurrent delta merge (torn-root split-brain).
                    let outcome =
                        apply_under_context_lock(context_client, context_id, &runtime_env, || {
                            apply_hc_leaf_gated(store, context_id, leaf_data, loaded_app_key)
                        })
                        .await?;
                    match outcome {
                        HcLeafGateOutcome::Buffered => {
                            // Declined: the leaf is buffered, not applied. Don't
                            // count it as merged and skip the bidirectional
                            // push-back — there's nothing newer to reconcile until
                            // this node advances its reader and the drain replays it.
                            continue;
                        }
                        HcLeafGateOutcome::SkippedStoreError => {
                            // Fail-closed: a store error prevented resolving the
                            // loaded reader, so we declined to apply (rather than
                            // applying ungated). The leaf is re-pushed next sync;
                            // skip the bidirectional push-back too.
                            continue;
                        }
                        HcLeafGateOutcome::Applied => {}
                    }
                    stats.entities_merged += 1;

                    // P3 (core#2716): a rotation-log entry-child just merged. Its
                    // anchor's `own_hash` folds the resolved writer set (Phase-2),
                    // but HC wrote the CHILD entity, not the anchor — so without
                    // re-folding here, `own_hash` still reflects the PRE-merge
                    // writers while the child's hash (in the anchor's `full_hash`)
                    // reflects the merged set. The two writer-set representations
                    // in the root desync and the root oscillates across HC rounds
                    // (the concurrent/multi-node non-convergence). Re-fold the
                    // anchor (entry-child → map parent → anchor) so the fold and
                    // the collection agree. Best-effort + under the context lock,
                    // same as the leaf apply above.
                    if matches!(
                        leaf_data.metadata.crdt_type,
                        calimero_primitives::crdt::CrdtType::RotationLog
                    ) {
                        let entry_id = calimero_storage::address::Id::new(leaf_data.key);
                        apply_under_context_lock(context_client, context_id, &runtime_env, || {
                            Interface::<MainStorage>::refold_anchor_for_rotation_child(entry_id);
                        })
                        .await;
                    }

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
                        get_local_tree_node(context_id, &remote_node.id, false, schema_app_key)
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

                // Tombstone reconciliation (symmetric clear convergence): the
                // remote advertises children it deleted. For any we still hold
                // live, apply the deletion (delete-wins by HLC) via the
                // authenticated DeleteRef path — so a peer that cleared an entry
                // converges us even when WE initiated the sync, without anyone
                // pushing the live entity. (Our own deletions flow the other way
                // via the remote_only → EntityDeletePush path below.)
                if !remote_node.deleted_children.is_empty() {
                    let applied =
                        apply_under_context_lock(context_client, context_id, &runtime_env, || {
                            apply_remote_tombstones(&remote_node.deleted_children)
                        })
                        .await;
                    if applied > 0 {
                        debug!(
                            %context_id,
                            applied,
                            "applied remote deleted_children (clear convergence)"
                        );
                    }
                }

                let local_version = with_runtime_env(runtime_env.clone(), || {
                    get_local_tree_node(
                        context_id,
                        &remote_node.id,
                        is_this_node_root,
                        schema_app_key,
                    )
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
                        // Remote-only children: the peer has them, we don't.
                        // Normally we recurse to pull them. But if we have a
                        // local tombstone for one (we cleared it), add-wins
                        // would wrongly resurrect it — instead propagate our
                        // deletion so the peer converges. The tombstone's
                        // `deleted_at`/`metadata` are carried so the peer
                        // applies it through the authenticated DeleteRef path.
                        for child_id in remote_only_children {
                            let tombstone = with_runtime_env(runtime_env.clone(), || {
                                let id = calimero_storage::address::Id::new(child_id);
                                match Index::<MainStorage>::get_index(id) {
                                    Ok(Some(idx)) => idx
                                        .deleted_at
                                        .map(|deleted_at| (deleted_at, idx.metadata.clone())),
                                    _ => None,
                                }
                            });
                            if let Some((deleted_at, metadata)) = tombstone {
                                pending_deletions.push(EntityDeletion {
                                    id: child_id,
                                    deleted_at,
                                    metadata,
                                });
                            } else {
                                to_compare.push((child_id, false));
                            }
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
                                schema_app_key,
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
                                collect_local_leaves(
                                    context_id,
                                    &local_node.id,
                                    is_this_node_root,
                                    schema_app_key,
                                )
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

    // Flush deletion propagation (clear/tombstone convergence). Children we
    // cleared but the peer still holds are pushed as authenticated tombstones
    // so the peer applies delete-wins instead of us silently re-pulling them.
    if !pending_deletions.is_empty() {
        let applied = push_deletions(
            transport,
            context_id,
            identity,
            &pending_deletions,
            &mut stats,
        )
        .await?;
        debug!(
            %context_id,
            tombstones = pending_deletions.len(),
            applied,
            "Flushed clear/tombstone deletion propagation"
        );
    }

    // Re-read the peer's CURRENT root before closing, so the convergence
    // check below compares against the peer's live state rather than the
    // root captured at handshake.
    //
    // The handshake root goes stale the moment either side moves: after a
    // bidirectional reconcile the initiator pushed local-only leaves /
    // tombstones, so the peer's root advanced past the captured value; the
    // peer may also have applied its own concurrent writes. Comparing a
    // converged session against that stale snapshot has two failure modes,
    // both observed in production:
    //   - false negative: a session that fully converged reports
    //     `root_hash_verified = false`, producing the WARN that fires on
    //     every interval-sync tick "forever";
    //   - false positive: a session that merely re-reached the stale
    //     snapshot reports `verified = true` while the peer has actually
    //     moved on, masking a real divergence.
    // Both collapse to "the guard never asked the peer where it is now."
    // One extra request closes the gap.
    //
    // Falls back to the captured handshake root when the peer does not
    // answer — an older peer closes the stream on the unrecognized request,
    // and a transport error is non-fatal here — preserving prior behaviour
    // for mixed-version clusters.
    // Reconcile per-Shared-entity rotation logs (core#2716/#2703) before the
    // root re-query and the close. A writer-set rotation is hash-neutral, so it
    // never flows through HC's hash-driven traversal; this end-of-session union
    // is what converges writer sets across a branch reconciled via HC.
    // Best-effort — an older peer or any transport hiccup must not fail the
    // session.
    if let Err(e) =
        reconcile_rotation_logs_with_peer(transport, context_id, identity, &runtime_env).await
    {
        debug!(%context_id, error = %e, "rotation-log reconciliation skipped (best-effort)");
    }

    let peer_current_root = match query_peer_current_root(transport, context_id, identity).await {
        Ok(Some(root)) => root,
        Ok(None) | Err(_) => remote_root_hash,
    };

    // Close the transport to signal completion to the responder
    transport.close().await?;

    let local_root_hash = with_runtime_env(runtime_env.clone(), || {
        get_local_root_hash_for_context(context_id)
    })?;
    stats.root_hash_verified = local_root_hash == peer_current_root;

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
            peer_hash = %hex::encode(&peer_current_root[..8]),
            nodes_compared = stats.nodes_compared,
            entities_merged = stats.entities_merged,
            entities_pushed = stats.entities_pushed,
            nodes_skipped = stats.nodes_skipped,
            "HashComparison sync did not converge with the peer's live root. \
             Compared against the peer's post-sync root (re-read at session end), \
             so a mismatch here means the two nodes are genuinely divergent — \
             persistent occurrences across interval-sync ticks indicate a real \
             merge convergence bug rather than benign handshake drift."
        );
    }

    Ok(stats)
}

/// Ask the peer for its current root hash at the end of a HashComparison
/// session, after both sides have applied every merge/push in this exchange.
///
/// Reuses the `DagHeadsRequest` / `DagHeadsResponse` pair (which already
/// carries the peer's live `root_hash`) so no new wire message is needed. The
/// initiator uses the returned root as the post-sync convergence target.
///
/// Returns `Ok(None)` when the peer closes the stream or replies with an
/// unexpected payload — an older peer that does not handle this mid-session
/// request — so the caller can fall back to the handshake root.
async fn query_peer_current_root<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    identity: PublicKey,
) -> Result<Option<[u8; 32]>> {
    let request = StreamMessage::Init {
        context_id,
        party_id: identity,
        payload: InitPayload::DagHeadsRequest { context_id },
        next_nonce: generate_nonce(),
    };
    transport.send(&request).await?;

    let Some(response) = transport.recv().await? else {
        return Ok(None);
    };

    match response {
        StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { root_hash, .. },
            ..
        } => Ok(Some(*root_hash)),
        _ => Ok(None),
    }
}

/// Cap on the number of per-`Shared`-entity rotation logs exchanged in one
/// reconciliation. A context has very few `Shared` anchors in practice; the cap
/// just bounds a pathological case (logged + truncated, never silently capped).
const MAX_ROTATION_LOGS_PER_SYNC: usize = 1024;

/// Walk the entity tree from the context root and collect each `Shared`
/// anchor's rotation log as `(entity_id, borsh(RotationLog))`.
///
/// Store keys are SHA-256 hashed (`Key::to_bytes`), so the rotation logs cannot
/// be prefix-scanned; they must be reached via the entity index, exactly like
/// [`collect_leaves_recursive`]. MUST run inside `with_runtime_env` so
/// `Index`/`rotation_log` route through this context's store.
pub(crate) fn collect_local_shared_rotation_logs(
    context_id: ContextId,
) -> Vec<([u8; 32], Vec<u8>)> {
    let mut out = Vec::new();
    collect_shared_rotation_logs_recursive(Id::new(*context_id.as_ref()), &mut out, 0);
    out
}

fn collect_shared_rotation_logs_recursive(
    entity_id: Id,
    out: &mut Vec<([u8; 32], Vec<u8>)>,
    depth: u32,
) {
    if depth >= MAX_COLLECT_DEPTH || out.len() >= MAX_ROTATION_LOGS_PER_SYNC {
        if out.len() >= MAX_ROTATION_LOGS_PER_SYNC {
            warn!(
                count = out.len(),
                "rotation-log sync: hit per-sync cap, truncating the set of \
                 exchanged Shared rotation logs"
            );
        }
        return;
    }

    let Ok(Some(index)) = Index::<MainStorage>::get_index(entity_id) else {
        return;
    };

    // Only `Shared` anchors own a rotation log; members/others don't.
    if matches!(index.metadata.storage_type, StorageType::Shared { .. }) {
        if let Ok(Some(log)) = rotation_log::load::<MainStorage>(entity_id) {
            if let Ok(bytes) = borsh::to_vec(&log) {
                out.push((*entity_id.as_bytes(), bytes));
            }
        }
    }

    if let Some(children) = index.children() {
        for child in children.iter() {
            collect_shared_rotation_logs_recursive(Id::new(*child.id().as_bytes()), out, depth + 1);
        }
    }
}

/// Union peer-supplied rotation logs into the local store.
///
/// For each `(entity_id, borsh(RotationLog))`, append every entry the local log
/// lacks via [`rotation_log::append`], which dedups by `delta_id`.
/// `resolve_local`/`writers_at` are order-invariant (max-by `(delta_hlc,
/// signer)`), so insertion order is irrelevant — the result is a set union.
/// Best-effort: a per-entity decode failure or a per-entry append failure
/// (e.g. a conflicting `delta_id`) is logged and skipped, never fatal. MUST run
/// inside `with_runtime_env`. Returns the number of append calls that succeeded
/// (includes idempotent no-ops).
pub(crate) fn union_received_rotation_logs(logs: &[([u8; 32], Vec<u8>)]) -> usize {
    let mut applied = 0_usize;
    for (entity_bytes, bytes) in logs {
        let entity_id = Id::new(*entity_bytes);
        let remote: RotationLog = match borsh::from_slice(bytes) {
            Ok(log) => log,
            Err(e) => {
                debug!(%entity_id, error = %e, "rotation-log sync: undecodable remote log, skipping");
                continue;
            }
        };
        let mut entity_applied = 0_usize;
        for entry in remote.entries {
            match rotation_log::append::<MainStorage>(entity_id, entry) {
                Ok(()) => {
                    applied += 1;
                    entity_applied += 1;
                }
                Err(e) => {
                    debug!(%entity_id, error = %e, "rotation-log sync: append skipped");
                }
            }
        }

        // Phase 2 of core#2716: the union just changed this anchor's resolved
        // writer/capability set WITHOUT an entity write, so its folded
        // `own_hash` (which now commits to the ACL) is stale. Recompute it and
        // propagate up the ancestor chain — otherwise the context root would
        // never reconverge after a reconcile and the cluster would split-brain
        // on a stable-but-different root (the dual of the bug the fold fixes).
        // Best-effort: a rehash failure is logged, not fatal — the next sync
        // round retries. Idempotent: re-delivering present entries recomputes
        // the same hash.
        if entity_applied > 0 {
            if let Err(e) = Interface::<MainStorage>::rehash_shared_anchor(entity_id) {
                debug!(
                    %entity_id,
                    error = %e,
                    "rotation-log sync: rehash_shared_anchor after union failed"
                );
            }
        }
    }
    applied
}

/// Reconcile per-`Shared`-entity rotation logs with the peer at the end of a
/// HashComparison session (core#2716/#2703).
///
/// HC reconciles entity *trees by hash* and prunes hash-equal subtrees, but a
/// writer-set rotation is **hash-neutral** (writers are `#[borsh(skip)]` out of
/// the Merkle hash), so a node that catches up to a peer's branch via HC never
/// learns the rotation — HC carries no rotation log. This one round-trip ships
/// each side's `Shared` rotation logs and unions the other's, so both converge
/// on the same writer set regardless of how the data was reconciled.
///
/// Best-effort and mixed-version safe: an older peer that doesn't understand
/// `RotationLogSyncRequest` errors/closes the stream, which surfaces here as an
/// `Err`/`None` and is swallowed by the caller — the session is unaffected.
///
/// Also driven standalone (on a fresh stream) by `SyncManager` from the
/// protocol-selection `None` path, where the Merkle roots already match but
/// hash-neutral rotations may still diverge (core#2716).
pub(crate) async fn reconcile_rotation_logs_with_peer<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    identity: PublicKey,
    runtime_env: &calimero_storage::env::RuntimeEnv,
) -> Result<()> {
    let local_logs = with_runtime_env(runtime_env.clone(), || {
        collect_local_shared_rotation_logs(context_id)
    });

    let request = StreamMessage::Init {
        context_id,
        party_id: identity,
        payload: InitPayload::RotationLogSyncRequest {
            context_id,
            logs: local_logs,
        },
        next_nonce: generate_nonce(),
    };
    transport.send(&request).await?;

    let Some(response) = transport.recv().await? else {
        return Ok(());
    };

    if let StreamMessage::Message {
        payload: MessagePayload::RotationLogSyncResponse { logs },
        ..
    } = response
    {
        let applied = with_runtime_env(runtime_env.clone(), || union_received_rotation_logs(&logs));
        if applied > 0 {
            debug!(
                %context_id,
                applied,
                "rotation-log reconciliation: unioned peer's Shared rotation logs"
            );
        }
    }

    Ok(())
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

    // PR-6b Task 6b.7: the sender's loaded-reader schema, stamped onto every
    // leaf we emit (see `run_initiator_impl`).
    let schema_app_key = calimero_context::hlc_fence::loaded_reader_app_key(store, &context_id)
        .ok()
        .flatten();

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
            get_local_tree_node(context_id, &first_node_id, is_root_request, schema_app_key)
        })?;

        let response = build_tree_node_response_internal(
            context_id,
            local_node,
            clamped_depth,
            &runtime_env,
            schema_app_key,
        )?;

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
                    get_local_tree_node(context_id, &node_id, is_root_request, schema_app_key)
                })?;

                let response = build_tree_node_response_internal(
                    context_id,
                    local_node,
                    clamped_depth,
                    &runtime_env,
                    schema_app_key,
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

                let outcome = handle_entity_push(store, &runtime_env, context_id, &entities);
                let applied = outcome.applied;

                // This responder runs without a `ContextClient` in
                // scope (trait signature limitation — see
                // `SyncProtocolExecutor`), so it can't dispatch
                // deferred root merges itself. The production
                // responder in `hash_comparison.rs` does have
                // `ContextClient` and dispatches. Surface the gap as
                // a warn so persistent occurrences are visible; in
                // practice the initiator's DFS catches the same root
                // divergence and dispatches from there.
                if !outcome.deferred_root_merges.is_empty() {
                    warn!(
                        %context_id,
                        deferred = outcome.deferred_root_merges.len(),
                        "EntityPush responder: dropped root-entity deferred merges \
                         (protocol-trait responder lacks ContextClient — initiator-side \
                         dispatch will pick up root divergence on next sync round)"
                    );
                }

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
                    deferred_root_merges = outcome.deferred_root_merges.len(),
                    total = entity_count,
                    "Applied pushed entities via CRDT merge"
                );
            }

            InitPayload::EntityDeletePush { deletions, .. } => {
                let total = deletions.len();
                trace!(%context_id, total, "Handling EntityDeletePush from initiator");

                // Apply each tombstone through the authenticated DeleteRef path
                // (delete-wins by HLC; signature/nonce verified for User/Shared
                // exactly as on the delta stream). A deletion that loses the LWW
                // race or fails authorization is a safe no-op — not counted.
                let mut applied: u32 = 0;
                for deletion in &deletions {
                    let action = calimero_storage::action::Action::DeleteRef {
                        id: calimero_storage::address::Id::new(deletion.id),
                        deleted_at: deletion.deleted_at,
                        metadata: deletion.metadata.clone(),
                    };
                    let result = with_runtime_env(runtime_env.clone(), || {
                        Interface::<MainStorage>::apply_action(
                            action,
                            &calimero_storage::interface::ApplyContext::empty(),
                        )
                    });
                    match result {
                        Ok(_) => applied += 1,
                        Err(e) => {
                            debug!(
                                %context_id,
                                id = %hex::encode(deletion.id),
                                error = %e,
                                "EntityDeletePush: skipped a tombstone (lost LWW or unauthorized)"
                            );
                        }
                    }
                }

                let msg = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::EntityDeletePushAck {
                        applied_count: applied,
                    },
                    next_nonce: generate_nonce(),
                };

                transport.send(&msg).await?;
                sequence_id += 1;
                requests_handled += 1;

                info!(%context_id, applied, total, "Applied pushed tombstones (delete-wins)");
            }

            InitPayload::DagHeadsRequest { .. } => {
                // End-of-session convergence re-read for the initiator's
                // post-sync check. Re-read our root NOW — after applying every
                // leaf/tombstone pushed in this session — instead of reusing
                // the value captured at the top of this responder, so the
                // initiator compares against our live post-merge state rather
                // than a stale snapshot.
                let current_root = with_runtime_env(runtime_env.clone(), || {
                    Index::<MainStorage>::get_hashes_for(Id::new(*context_id.as_ref()))
                        .ok()
                        .flatten()
                        .map(|(full, _)| full)
                        .unwrap_or([0; 32])
                });

                let msg = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::DagHeadsResponse {
                        dag_heads: Vec::new(),
                        root_hash: Hash::from(current_root),
                    },
                    next_nonce: generate_nonce(),
                };

                transport.send(&msg).await?;
                sequence_id += 1;
                requests_handled += 1;
            }

            InitPayload::RotationLogSyncRequest { logs, .. } => {
                // Union the initiator's Shared rotation logs into ours, then
                // reply with our own so the initiator unions them too — one
                // round-trip reconciles both directions (core#2716/#2703).
                let applied =
                    with_runtime_env(runtime_env.clone(), || union_received_rotation_logs(&logs));
                let local_logs = with_runtime_env(runtime_env.clone(), || {
                    collect_local_shared_rotation_logs(context_id)
                });

                let msg = StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::RotationLogSyncResponse { logs: local_logs },
                    next_nonce: generate_nonce(),
                };

                transport.send(&msg).await?;
                sequence_id += 1;
                requests_handled += 1;

                if applied > 0 {
                    info!(%context_id, applied, "rotation-log sync: unioned initiator's Shared rotation logs");
                }
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
    schema_app_key: Option<[u8; 32]>,
) -> Result<TreeNodeResponse> {
    let response = if let Some(node) = local_node {
        let mut nodes = vec![node.clone()];

        // Include children if depth > 0
        let depth = clamped_depth.unwrap_or(0);
        if depth > 0 && node.is_internal() {
            for child_id in &node.children {
                if let Some(child) = with_runtime_env(runtime_env.clone(), || {
                    get_local_tree_node(context_id, child_id, false, schema_app_key)
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
    schema_app_key: Option<[u8; 32]>,
) -> Result<Vec<TreeLeafData>> {
    let mut leaves = Vec::new();
    collect_leaves_recursive(context_id, node_id, is_root, &mut leaves, 0, schema_app_key)?;
    Ok(leaves)
}

/// Recursively collect leaf data from a subtree.
///
/// `schema_app_key` (PR-6b Task 6b.7): stamped onto each emitted leaf — see
/// [`get_local_tree_node`].
fn collect_leaves_recursive(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root: bool,
    leaves: &mut Vec<TreeLeafData>,
    depth: u32,
    schema_app_key: Option<[u8; 32]>,
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
            // Ship the full ancestor chain alongside `parent_id`. Same
            // trust model as the existing `parent_id` wire — not
            // cryptographically signed; HashComparison sync exists to
            // repair drifted tree shapes, so a signed commitment to a
            // single shape would reject every legitimate repair. See the
            // `LeafMetadata::ancestors` field doc for why this matters
            // for nested entities (without the chain the receiver's
            // ancestor loop calls `add_root` for any missing
            // grandparent, misplacing the subtree).
            if let Ok(ancestors) = Index::<MainStorage>::get_ancestors_of(entity_id) {
                metadata = metadata.with_ancestors(ancestors);
            }
            if let Some(auth) = crate::sync::helpers::wire_authorization_for(&index.metadata) {
                metadata = metadata.with_authorization(auth);
            }
            // PR-6b Task 6b.7: stamp the sender's loaded-reader schema — see
            // `get_local_tree_node`.
            if let Some(schema) = schema_app_key {
                metadata = metadata.with_schema_app_key(schema);
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
            collect_leaves_recursive(
                context_id,
                child_id,
                false,
                leaves,
                depth + 1,
                schema_app_key,
            )?;
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
    schema_app_key: Option<[u8; 32]>,
) -> Result<u64> {
    let mut total = 0u64;

    // Flush per-subtree to avoid accumulating all leaves in memory
    for child_id in local_only_children {
        let leaves = with_runtime_env(runtime_env.clone(), || {
            collect_local_leaves(context_id, child_id, false, schema_app_key)
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

/// Push tombstones to the peer via `EntityDeletePush` messages (batched).
///
/// Propagates deletions for entities the local node cleared but the peer still
/// holds. The peer applies each through the authenticated `Action::DeleteRef`
/// path (delete-wins by HLC), so a deletion converges instead of being
/// resurrected by HashComparison's add-wins child comparison.
async fn push_deletions<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    identity: PublicKey,
    deletions: &[EntityDeletion],
    stats: &mut HashComparisonStats,
) -> Result<u64> {
    let mut total_applied = 0u64;

    for chunk in deletions.chunks(MAX_ENTITIES_PER_PUSH) {
        let push_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            payload: InitPayload::EntityDeletePush {
                context_id,
                deletions: chunk.to_vec(),
            },
            next_nonce: generate_nonce(),
        };

        transport.send(&push_msg).await?;
        stats.requests_sent += 1;

        let ack = transport
            .recv()
            .await?
            .ok_or_else(|| eyre::eyre!("stream closed while waiting for EntityDeletePushAck"))?;

        match ack {
            StreamMessage::Message {
                payload: MessagePayload::EntityDeletePushAck { applied_count },
                ..
            } => {
                total_applied += u64::from(applied_count);
            }
            _ => {
                bail!(
                    "Unexpected response to EntityDeletePush (peer may not support tombstone propagation)"
                );
            }
        }
    }

    Ok(total_applied)
}

// =============================================================================
// Local Tree Node Lookup
// =============================================================================

/// Get a tree node from the local Merkle tree Index.
///
/// `schema_app_key` (PR-6b Task 6b.7): the sender's loaded-reader app-schema
/// key, stamped onto each emitted leaf so a receiver on an older reader can
/// decline+buffer a future-schema leaf. `None` when the sender can't resolve
/// its loaded reader (parity with the receiver's no-gate fallback).
fn get_local_tree_node(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root_request: bool,
    schema_app_key: Option<[u8; 32]>,
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

    // Tombstones for children this node removed, resolved to signed
    // `EntityDeletion`s from each child's own tombstone index. Carried on the
    // wire so a peer that still holds the child converges to the deletion
    // (delete-wins) during comparison — without anyone pushing the live entity.
    let deleted_children = collect_deleted_children_wire(&index);

    // A node with live children and/or tombstoned children is INTERNAL. A
    // collection cleared to childless still carries `deleted_children`, so emit
    // it as internal carrying the tombstones (not as a leaf) — that's what lets
    // the deletion propagate. Behaviour is unchanged when there are no
    // tombstones (`deleted_children` empty): the old leaf/internal split below.
    if !children_ids.is_empty() || !deleted_children.is_empty() {
        let mut node = TreeNode::internal(*entity_id.as_bytes(), full_hash, children_ids);
        node.deleted_children = deleted_children;
        return Ok(Some(node));
    }

    // No children, live or tombstoned — leaf, or empty-internal.
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
        // Full ancestor chain — see the matching block in
        // `collect_leaves_recursive` for rationale.
        if let Ok(ancestors) = Index::<MainStorage>::get_ancestors_of(entity_id) {
            metadata = metadata.with_ancestors(ancestors);
        }
        if let Some(auth) = crate::sync::helpers::wire_authorization_for(&index.metadata) {
            metadata = metadata.with_authorization(auth);
        }
        // PR-6b Task 6b.7: stamp the sender's loaded-reader schema so a receiver
        // on an older reader can decline+buffer this leaf if it's future-schema.
        if let Some(schema) = schema_app_key {
            metadata = metadata.with_schema_app_key(schema);
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
}

/// Apply tombstones a remote node advertised in its `deleted_children`, for any
/// entity we still hold live. Each goes through the authenticated
/// `Action::DeleteRef` path (delete-wins by HLC; signature/nonce verified for
/// User/Shared, safe no-op when it loses or fails auth). Returns the count
/// applied. Must be called inside a `with_runtime_env` scope.
pub(crate) fn apply_remote_tombstones(deletions: &[EntityDeletion]) -> u64 {
    let mut applied = 0u64;
    for deletion in deletions {
        let action = calimero_storage::action::Action::DeleteRef {
            id: Id::new(deletion.id),
            deleted_at: deletion.deleted_at,
            metadata: deletion.metadata.clone(),
        };
        if Interface::<MainStorage>::apply_action(
            action,
            &calimero_storage::interface::ApplyContext::empty(),
        )
        .is_ok()
        {
            applied += 1;
        }
    }
    applied
}

/// Resolve a node's `deleted_children` (child ids) to signed `EntityDeletion`s
/// for the wire, reading each child's own tombstone index for `deleted_at` +
/// the (signed) metadata. Entries whose child index is gone (GC'd) or not
/// actually tombstoned are skipped.
pub(crate) fn collect_deleted_children_wire(
    index: &calimero_storage::index::EntityIndex,
) -> Vec<EntityDeletion> {
    index
        .deleted_children()
        .iter()
        .filter_map(|child_id| {
            let cidx = Index::<MainStorage>::get_index(*child_id).ok().flatten()?;
            let deleted_at = cidx.deleted_at?;
            Some(EntityDeletion {
                id: *child_id.as_bytes(),
                deleted_at,
                metadata: cidx.metadata.clone(),
            })
        })
        .collect()
}

/// Outcome of gating a single HashComparison sync-repair leaf against the
/// receiver's loaded reader (PR-6b Task 6b.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HcLeafGateOutcome {
    /// The leaf was applied to storage.
    Applied,
    /// The leaf was declined + buffered (its schema is newer than the loaded
    /// reader); the DFS skips it until the drain replays it.
    Buffered,
    /// The loaded reader could not be resolved (store error). Fail CLOSED: the
    /// leaf was NOT applied; the DFS skips it and it is re-pushed next cycle.
    SkippedStoreError,
}

/// Apply (or decline+buffer) a single HC sync-repair leaf, gating on the
/// receiver's loaded-reader schema.
///
/// `loaded_app_key` is the resolution of the receiver's loaded reader schema —
/// the FULL `Result`, not collapsed with `.ok().flatten()`, so the three gate
/// states stay distinct:
/// * `Ok(Some(k))` — gate active; a future-schema leaf is declined+buffered.
/// * `Ok(None)` — legitimately no group / unresolvable meta ⇒ no gate, apply as
///   today.
/// * `Err(_)` — a STORE ERROR; readability cannot be determined. Fail CLOSED:
///   warn and SKIP the leaf ([`HcLeafGateOutcome::SkippedStoreError`]) rather
///   than applying ungated, which would let a future-schema leaf the node can't
///   read get LWW-stored (the v1-binary-fed-v2-bytes corruption this gate
///   prevents). HC repair leaves are non-destructive and are re-pushed on the
///   next sync cycle, so skipping here is safe.
///
/// Must be called inside the per-context execution lock + `with_runtime_env`
/// scope (it delegates to the apply helpers).
fn apply_hc_leaf_gated(
    store: &Store,
    context_id: ContextId,
    leaf: &TreeLeafData,
    loaded_app_key: Result<Option<[u8; 32]>>,
) -> Result<HcLeafGateOutcome> {
    let loaded_app_key = match loaded_app_key {
        Ok(key) => key,
        Err(e) => {
            warn!(
                %context_id,
                error = %e,
                key = %hex::encode(leaf.key),
                "HC merge: could not resolve loaded reader schema (store error); \
                 skipping leaf fail-closed — it will be re-pushed next sync"
            );
            return Ok(HcLeafGateOutcome::SkippedStoreError);
        }
    };

    match loaded_app_key {
        Some(loaded) => Ok(
            match apply_leaf_with_crdt_merge_gated(store, context_id, leaf, loaded)? {
                LeafOutcome::Applied => HcLeafGateOutcome::Applied,
                LeafOutcome::Buffered => HcLeafGateOutcome::Buffered,
            },
        ),
        None => {
            apply_leaf_with_crdt_merge(context_id, leaf)?;
            Ok(HcLeafGateOutcome::Applied)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let config = HashComparisonConfig {
            remote_root_hash: [1u8; 32],
            context_client: None,
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

            let node = get_local_tree_node(context_id, opaque_id.as_bytes(), false, None)
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

    /// Regression guard for the frozen-storage HashComparison split-brain.
    ///
    /// `apply_leaf_with_crdt_merge` previously emitted `Action::Update` for
    /// ANY entity that already existed locally — including `Frozen` ones.
    /// The storage layer categorically rejects `Update` for `Frozen`
    /// ("Frozen data cannot be updated"), so re-applying an already-present
    /// frozen leaf (which a bulk leaf push does while repairing a divergent
    /// sibling) aborted the ENTIRE repair and left the context permanently
    /// split-brained. Frozen entries are content-addressed + immutable, so
    /// an already-present one must be skipped, not updated. This test pins
    /// that: re-applying an existing frozen leaf is a no-op success.
    #[test]
    fn apply_leaf_skips_existing_frozen_entry() {
        use std::sync::Arc;

        use calimero_node_primitives::sync::hash_comparison::{LeafMetadata, TreeLeafData};
        use calimero_primitives::crdt::CrdtType;
        use calimero_storage::action::Action;
        use calimero_storage::entities::{ChildInfo, Metadata, StorageType};
        use calimero_storage::interface::ApplyContext;
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;
        use sha2::{Digest, Sha256};

        use crate::sync::helpers::apply_leaf_with_crdt_merge;

        let context_id = ContextId::from([0xCA; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        let root_id = Id::new(*context_id.as_ref());
        let frozen_id = Id::new([0x42u8; 32]);

        // Frozen content-addressed blob: [key_hash(32)][value][element_id(32)].
        let value = b"immutable-frozen-payload".to_vec();
        let key_hash: [u8; 32] = Sha256::digest(&value).into();
        let mut blob = Vec::new();
        blob.extend_from_slice(&key_hash);
        blob.extend_from_slice(&value);
        blob.extend_from_slice(frozen_id.as_bytes());

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

            // Seed a Frozen entry as a child of root.
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

            let mut frozen_meta = Metadata::new(100, 100);
            frozen_meta.storage_type = StorageType::Frozen;
            frozen_meta.crdt_type = Some(CrdtType::FrozenStorage);

            Interface::<MainStorage>::apply_action(
                Action::Add {
                    id: frozen_id,
                    data: blob.clone(),
                    ancestors: vec![ChildInfo::new(root_id, root_hash, root_meta)],
                    metadata: frozen_meta,
                },
                &ApplyContext::empty(),
            )
            .expect("seed frozen entry");

            // A peer re-pushes the SAME frozen leaf (as happens during a
            // bulk leaf push while repairing a sibling). Frozen leaves carry
            // no wire authorization, so apply_leaf resolves storage_type from
            // the existing (Frozen) entry and would otherwise emit Update.
            let leaf = TreeLeafData::new(
                *frozen_id.as_bytes(),
                blob.clone(),
                LeafMetadata::new(CrdtType::FrozenStorage, 100, *root_id.as_bytes()),
            );

            // Before the fix this returned Err("Frozen data cannot be
            // updated"); after the fix it is a no-op success.
            apply_leaf_with_crdt_merge(context_id, &leaf)
                .expect("re-applying an existing frozen leaf must be a no-op, not a fatal Update");

            // The frozen entry is still present and unchanged.
            assert!(
                Index::<MainStorage>::get_index(frozen_id)
                    .unwrap()
                    .is_some(),
                "frozen entry must remain present after the skipped re-apply"
            );
        });
    }

    /// A *new* Frozen entity pushed as a bare HC leaf must land as
    /// `StorageType::Frozen`, not `Public`.
    ///
    /// Frozen entities carry no wire authorization (`wire_authorization_for`
    /// returns None for Frozen), so before the fix a freshly-received frozen
    /// leaf defaulted to `Public` (its `crdt_type` was set to `FrozenStorage`
    /// but `storage_type` stayed `Public`). A later real `Frozen` delta then hit
    /// `apply_action`'s guard with `existing=Public new=Frozen` →
    /// `ActionNotAllowed("Cannot change StorageType")`, panicking the guest's
    /// frozen-value merge (the HC/LevelWise frozen-push split-brain, #2591).
    /// `apply_leaf_with_crdt_merge` now infers `Frozen` from the wire-carried
    /// `crdt_type`.
    #[test]
    fn apply_leaf_new_frozen_entry_lands_as_frozen_not_public() {
        use std::sync::Arc;

        use calimero_node_primitives::sync::hash_comparison::{LeafMetadata, TreeLeafData};
        use calimero_primitives::crdt::CrdtType;
        use calimero_storage::action::Action;
        use calimero_storage::entities::{Metadata, StorageType};
        use calimero_storage::interface::ApplyContext;
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;
        use sha2::{Digest, Sha256};

        use crate::sync::helpers::apply_leaf_with_crdt_merge;

        let context_id = ContextId::from([0xCC; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        let root_id = Id::new(*context_id.as_ref());
        let frozen_id = Id::new([0x77u8; 32]);

        // Frozen content-addressed blob: [key_hash(32)][value][element_id(32)].
        let value = b"freshly-pushed-frozen".to_vec();
        let key_hash: [u8; 32] = Sha256::digest(&value).into();
        let mut blob = Vec::new();
        blob.extend_from_slice(&key_hash);
        blob.extend_from_slice(&value);
        blob.extend_from_slice(frozen_id.as_bytes());

        with_runtime_env(runtime_env.clone(), || {
            // Context root only — the frozen entity does NOT exist locally yet.
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

            // A bare frozen leaf as HC ships it: crdt_type=FrozenStorage, the
            // root as parent, and NO wire authorization (Frozen carries none).
            let leaf = TreeLeafData::new(
                *frozen_id.as_bytes(),
                blob.clone(),
                LeafMetadata::new(CrdtType::FrozenStorage, 100, *root_id.as_bytes())
                    .with_parent(*root_id.as_bytes()),
            );
            apply_leaf_with_crdt_merge(context_id, &leaf).expect("apply new frozen leaf");

            let md = Index::<MainStorage>::get_index(frozen_id)
                .unwrap()
                .expect("frozen entity should have been created")
                .metadata;
            assert!(
                matches!(md.storage_type, StorageType::Frozen),
                "new frozen leaf must land as Frozen, not {:?} — else a later Frozen \
                 delta is rejected with Cannot change StorageType",
                md.storage_type
            );
        });
    }

    /// Characterization guard that isolates the `clear()` HashComparison
    /// split-brain to delete *propagation*, NOT resurrection.
    ///
    /// When a node has cleared an entry but a peer still holds it, HC fetches
    /// the peer's live copy and re-applies it through `apply_leaf_with_crdt_merge`
    /// (the same entrypoint the bidirectional reconcile uses for a child the
    /// peer has and we don't). This pins that the cleared node correctly
    /// REFUSES to resurrect it: the tombstone's high-water `updated_at` (stamped
    /// by `remove_child_from` at the clear) is strictly newer than the peer's
    /// value (`updated_at = 100`), so delete-wins keeps it deleted.
    ///
    /// So the apply side is safe — the clear split-brain is solely that the
    /// deletion never *reaches* the peer that kept the entry (HC carries no
    /// tombstone). That non-convergence is reproduced end-to-end against the
    /// real protocol by the sim test
    /// `sync_sim::protocol::tests::test_hashcomparison_propagates_clear_tombstone`.
    #[test]
    fn hashcomparison_pull_does_not_resurrect_cleared_entry() {
        use std::sync::Arc;

        use calimero_node_primitives::sync::hash_comparison::{LeafMetadata, TreeLeafData};
        use calimero_primitives::crdt::CrdtType;
        use calimero_storage::action::Action;
        use calimero_storage::entities::{ChildInfo, Metadata};
        use calimero_storage::interface::ApplyContext;
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;

        use crate::sync::helpers::apply_leaf_with_crdt_merge;

        let context_id = ContextId::from([0xCB; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        let root_id = Id::new(*context_id.as_ref());
        let entry_id = Id::new([0x77u8; 32]);

        let child_ids = |parent: Id| -> Vec<Id> {
            Index::<MainStorage>::get_children_of(parent)
                .unwrap_or_default()
                .iter()
                .map(ChildInfo::id)
                .collect()
        };

        with_runtime_env(runtime_env.clone(), || {
            // Context root.
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

            // Seed an LWW entry under root, written at hlc 100.
            let mut md = Metadata::new(100, 100);
            md.crdt_type = Some(CrdtType::LwwRegister {
                inner_type: "String".to_owned(),
            });
            Interface::<MainStorage>::apply_action(
                Action::Add {
                    id: entry_id,
                    data: b"peer-value".to_vec(),
                    ancestors: vec![ChildInfo::new(root_id, root_hash, root_meta)],
                    metadata: md,
                },
                &ApplyContext::empty(),
            )
            .expect("seed entry");
            assert!(
                child_ids(root_id).contains(&entry_id),
                "entry should be seeded under root"
            );

            // CLEAR: delete the entry locally. `remove_child_from` stamps a
            // tombstone (deleted_at = time_now(), >> 100), drops it from root's
            // children, and would broadcast a DeleteRef on the delta path.
            Interface::<MainStorage>::remove_child_from(root_id, entry_id).expect("clear entry");
            assert!(
                Index::<MainStorage>::is_deleted(entry_id).unwrap(),
                "entry must be tombstoned after clear"
            );
            assert!(
                !child_ids(root_id).contains(&entry_id),
                "cleared entry must leave root's children"
            );

            // HASHCOMPARISON PULL: a peer that never saw the delete still holds
            // the entry. HC fetches it as a leaf and re-applies it with the
            // peer's (older) hlc=100 — the real HC apply entrypoint.
            let leaf = TreeLeafData::new(
                *entry_id.as_bytes(),
                b"peer-value".to_vec(),
                LeafMetadata::new(
                    CrdtType::LwwRegister {
                        inner_type: "String".to_owned(),
                    },
                    100,
                    *root_id.as_bytes(),
                ),
            );
            apply_leaf_with_crdt_merge(context_id, &leaf).expect("apply pulled leaf");

            // DELETE-WINS: our deletion is newer than the pulled value, so the
            // cleared entry MUST stay deleted. Today HC has no tombstone
            // awareness, so it is resurrected and these assertions fail.
            assert!(
                Index::<MainStorage>::is_deleted(entry_id).unwrap(),
                "HashComparison pull resurrected a cleared entry (tombstone lost) — \
                 delete-wins violated, so the deletion can never converge"
            );
            assert!(
                !child_ids(root_id).contains(&entry_id),
                "HashComparison pull re-added the cleared entry to root's children — \
                 root hash diverges from a peer that applied the delete"
            );
        });
    }

    // ---- PR-6b fail-closed (cursor): the HC DFS leaf-merge gate must NOT be
    //      disabled by a store error resolving the loaded reader. The old
    //      `.ok().flatten()` collapsed `Err` into `None` (ungated apply). ----

    use calimero_node_primitives::sync::hash_comparison::{LeafMetadata, TreeLeafData};
    use calimero_primitives::crdt::CrdtType;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use std::sync::Arc;

    fn hc_opaque_leaf(key: [u8; 32], schema: Option<[u8; 32]>) -> TreeLeafData {
        let mut md = LeafMetadata::new(CrdtType::lww_register("test"), 100, [0u8; 32]);
        if let Some(k) = schema {
            md = md.with_schema_app_key(k);
        }
        TreeLeafData::new(key, b"v2-bytes".to_vec(), md)
    }

    #[test]
    fn hc_store_error_resolving_gate_skips_leaf_not_applies() {
        // A transient store error while resolving the loaded reader on the HC
        // DFS apply path must fail CLOSED: the leaf is skipped (re-pushed next
        // sync), NOT applied ungated. The old `.ok().flatten()` collapsed `Err`
        // into `None` and would have LWW-stored the (possibly future-schema)
        // leaf — the v1-binary-fed-v2-bytes corruption hazard.
        let context_id = ContextId::from([0xCD; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        let leaf_key = [0x45u8; 32];
        // No schema marker — under `Ok(None)` (no gate) this WOULD apply, so the
        // only thing keeping it out of storage is the fail-closed skip.
        let leaf = hc_opaque_leaf(leaf_key, None);

        let outcome = with_runtime_env(runtime_env.clone(), || {
            apply_hc_leaf_gated(
                &store,
                context_id,
                &leaf,
                Err(eyre::eyre!("simulated transient store error")),
            )
        })
        .expect("gate must not propagate the store error");

        assert_eq!(
            outcome,
            HcLeafGateOutcome::SkippedStoreError,
            "fail-closed: a store error must skip the leaf, not apply it"
        );

        let stored = with_runtime_env(runtime_env.clone(), || {
            Index::<MainStorage>::get_index(Id::new(leaf_key))
                .ok()
                .flatten()
        });
        assert!(
            stored.is_none(),
            "store error must NOT result in an ungated apply/store"
        );
    }

    #[test]
    fn hc_no_gate_ok_none_still_applies_leaf() {
        // Distinct from the `Err` case: `Ok(None)` is the legitimate "no group /
        // unresolvable meta" case and MUST still apply as today.
        let context_id = ContextId::from([0xCE; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = create_runtime_env(&store, context_id, identity);

        let leaf_key = [0x46u8; 32];
        let leaf = hc_opaque_leaf(leaf_key, None);

        let outcome = with_runtime_env(runtime_env.clone(), || {
            apply_hc_leaf_gated(&store, context_id, &leaf, Ok(None))
        })
        .expect("gate must apply the leaf");

        assert_eq!(
            outcome,
            HcLeafGateOutcome::Applied,
            "Ok(None) legitimate-no-gate case must apply the leaf"
        );

        let stored = with_runtime_env(runtime_env.clone(), || {
            Index::<MainStorage>::get_index(Id::new(leaf_key))
                .ok()
                .flatten()
        });
        assert!(
            stored.is_some(),
            "Ok(None) no-gate case must store the leaf"
        );
    }

    /// core#2716/#2703: the rotation-log reconciliation helpers converge two
    /// nodes whose `Shared` rotation logs diverged because one only learned a
    /// hash-neutral rotation via HashComparison (which carries no rotation log).
    /// `collect_local_shared_rotation_logs` walks the entity index to find the
    /// `Shared` anchor and serialise its log; `union_received_rotation_logs`
    /// appends the missing entries (dedup by `delta_id`) so `resolve_local`
    /// converges on the same writer set.
    #[test]
    fn rotation_log_reconciliation_converges_divergent_shared_logs() {
        use core::num::NonZeroU128;
        use std::collections::{BTreeMap, BTreeSet};
        use std::sync::Arc;

        use calimero_storage::action::Action;
        use calimero_storage::entities::{ChildInfo, Metadata};
        use calimero_storage::interface::ApplyContext;
        use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        use calimero_storage::tests::common::{build_signed_shared_action, pubkey_of};
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;
        use ed25519_dalek::SigningKey;

        fn hlc(ns: u64) -> HybridTimestamp {
            HybridTimestamp::new(Timestamp::new(
                NTP64(ns),
                ID::from(NonZeroU128::new(1).unwrap()),
            ))
        }

        let context_id = ContextId::from([0xC7; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let anchor_id = Id::new([0x77; 32]);

        let alice_sk = SigningKey::from_bytes(&[0xA1; 32]);
        let alice = pubkey_of(&alice_sk);
        let bob = pubkey_of(&SigningKey::from_bytes(&[0xB2; 32]));
        let carol = pubkey_of(&SigningKey::from_bytes(&[0xC3; 32]));
        let genesis: BTreeSet<PublicKey> = [alice, bob].into_iter().collect();

        // Seed a node: a `Shared` anchor bootstrapped {Alice, Bob} as a child of
        // the context root, optionally followed by a later {Alice, Carol}
        // rotation (the hash-neutral grant a peer would only see if it applied
        // the rotation as a delta).
        let seed = |with_rotation: bool| -> Store {
            let store = Store::new(Arc::new(InMemoryDB::owned()));
            let env = create_runtime_env(&store, context_id, identity);
            with_runtime_env(env, || {
                let root_id = Id::new(*context_id.as_ref());
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

                let bootstrap = build_signed_shared_action(
                    true,
                    anchor_id,
                    b"v0".to_vec(),
                    genesis.clone(),
                    10,
                    &alice_sk,
                    vec![ChildInfo::new(root_id, root_hash, root_meta)],
                );
                Interface::<MainStorage>::apply_action(
                    bootstrap,
                    &ApplyContext {
                        effective_writers: Some(calimero_storage::entities::full_mask(
                            genesis.clone(),
                        )),
                        delta_id: Some([0xE0; 32]),
                        delta_hlc: Some(hlc(10)),
                    },
                )
                .expect("bootstrap shared anchor");

                if with_rotation {
                    let rotation = build_signed_shared_action(
                        false,
                        anchor_id,
                        b"v0".to_vec(),
                        [alice, carol].into_iter().collect(),
                        30,
                        &alice_sk,
                        vec![],
                    );
                    Interface::<MainStorage>::apply_action(
                        rotation,
                        &ApplyContext {
                            effective_writers: Some(calimero_storage::entities::full_mask(
                                genesis.clone(),
                            )),
                            delta_id: Some([0xE1; 32]),
                            delta_hlc: Some(hlc(30)),
                        },
                    )
                    .expect("rotation to {Alice, Carol}");
                }
            });
            store
        };

        let full = seed(true);
        let hc = seed(false);
        let full_env = create_runtime_env(&full, context_id, identity);
        let hc_env = create_runtime_env(&hc, context_id, identity);

        let resolve = |env: &calimero_storage::env::RuntimeEnv| -> BTreeMap<PublicKey, calimero_storage::entities::OpMask> {
            with_runtime_env(env.clone(), || {
                rotation_log::resolve_local(
                    &rotation_log::load::<MainStorage>(anchor_id)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap()
            })
        };

        // Read the anchor's stored own_hash (Phase 2 folds the resolved ACL in).
        let anchor_own_hash = |env: &calimero_storage::env::RuntimeEnv| -> [u8; 32] {
            with_runtime_env(env.clone(), || {
                Index::<MainStorage>::get_hashes_for(anchor_id)
                    .unwrap()
                    .unwrap()
                    .1
            })
        };

        // Precondition: the HC node has NOT learned Carol (it only has bootstrap).
        let hc_before = resolve(&hc_env);
        assert!(
            !hc_before.contains_key(&carol),
            "precondition: HC node lacks Carol; got {hc_before:?}"
        );

        // Phase 2 (core#2716): the ACL is folded into the anchor's own_hash, so
        // the divergent writer sets MUST surface as divergent anchor hashes
        // BEFORE reconcile — otherwise a matching root would hide the ACL
        // divergence (the hash-neutral split-brain this fold retires).
        let full_own_before = anchor_own_hash(&full_env);
        let hc_own_before = anchor_own_hash(&hc_env);
        assert_ne!(
            full_own_before, hc_own_before,
            "Phase 2: divergent writer sets must produce divergent anchor own_hash"
        );

        // Collect the full node's Shared rotation logs (the wire payload) — the
        // index walk must find the anchor.
        let collected = with_runtime_env(full_env.clone(), || {
            collect_local_shared_rotation_logs(context_id)
        });
        assert!(
            collected.iter().any(|(id, _)| *id == *anchor_id.as_bytes()),
            "collect_local_shared_rotation_logs must find the Shared anchor"
        );

        // Union into the HC node and assert it converges on {Alice, Carol}.
        let applied = with_runtime_env(hc_env.clone(), || union_received_rotation_logs(&collected));
        assert!(applied > 0, "union must append the missing rotation entry");

        let hc_after = resolve(&hc_env);
        let full_writers = resolve(&full_env);
        assert_eq!(
            hc_after, full_writers,
            "after the union both nodes resolve the same writer set"
        );
        assert!(
            hc_after.contains_key(&carol),
            "HC node now recognises Carol as a writer; got {hc_after:?}"
        );

        // Phase 2 (core#2716): `union_received_rotation_logs` re-hashes the
        // anchor (`rehash_shared_anchor`), so the folded own_hash must now match
        // the full node's — the context root reconverges and there is no
        // stable-but-different-root split-brain (the dual bug the fold could
        // otherwise introduce on the union path).
        let hc_own_after = anchor_own_hash(&hc_env);
        assert_eq!(
            hc_own_after, full_own_before,
            "after the union the HC node's anchor own_hash must match the full node's"
        );
    }

    /// Phase 2 leg 4 (core#2716): the originator records its OWN rotation via
    /// `self_log_and_rehash_own_rotations` (the execute pipeline's post-delta
    /// step), the receiver via `apply_action`. Both must land the *same* anchor
    /// `own_hash` — otherwise the author of a rotation never converges with the
    /// peers it ships the rotation to.
    #[test]
    fn originator_self_log_and_rehash_matches_receiver_apply_action() {
        use core::num::NonZeroU128;
        use std::collections::BTreeSet;
        use std::sync::Arc;

        use calimero_storage::action::Action;
        use calimero_storage::entities::{full_mask, ChildInfo, Metadata};
        use calimero_storage::interface::ApplyContext;
        use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        use calimero_storage::tests::common::{build_signed_shared_action, pubkey_of};
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;
        use ed25519_dalek::SigningKey;

        fn hlc(ns: u64) -> HybridTimestamp {
            HybridTimestamp::new(Timestamp::new(
                NTP64(ns),
                ID::from(NonZeroU128::new(1).unwrap()),
            ))
        }

        let context_id = ContextId::from([0xC8; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let anchor_id = Id::new([0x88; 32]);
        let rotation_delta_id = [0xE1; 32];

        let alice_sk = SigningKey::from_bytes(&[0xA1; 32]);
        let alice = pubkey_of(&alice_sk);
        let bob = pubkey_of(&SigningKey::from_bytes(&[0xB2; 32]));
        let carol = pubkey_of(&SigningKey::from_bytes(&[0xC3; 32]));
        let genesis: BTreeSet<PublicKey> = [alice, bob].into_iter().collect();
        let rotated: BTreeSet<PublicKey> = [alice, carol].into_iter().collect();

        // Bootstrap a `Shared` anchor {Alice,Bob} under the context root.
        let bootstrap = |store: &Store| {
            let env = create_runtime_env(store, context_id, identity);
            with_runtime_env(env, || {
                let root_id = Id::new(*context_id.as_ref());
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
                let (root_hash, _) = Index::<MainStorage>::get_hashes_for(root_id)
                    .ok()
                    .flatten()
                    .unwrap_or(([0; 32], [0; 32]));
                let root_meta = Index::<MainStorage>::get_index(root_id)
                    .ok()
                    .flatten()
                    .map(|idx| idx.metadata.clone())
                    .unwrap_or_default();
                Interface::<MainStorage>::apply_action(
                    build_signed_shared_action(
                        true,
                        anchor_id,
                        b"v0".to_vec(),
                        genesis.clone(),
                        10,
                        &alice_sk,
                        vec![ChildInfo::new(root_id, root_hash, root_meta)],
                    ),
                    &ApplyContext {
                        effective_writers: Some(full_mask(genesis.clone())),
                        delta_id: Some([0xE0; 32]),
                        delta_hlc: Some(hlc(10)),
                    },
                )
                .expect("bootstrap shared anchor");
            });
        };

        let own_hash = |store: &Store| -> [u8; 32] {
            let env = create_runtime_env(store, context_id, identity);
            with_runtime_env(env, || {
                Index::<MainStorage>::get_hashes_for(anchor_id)
                    .unwrap()
                    .unwrap()
                    .1
            })
        };

        // The rotation {Alice,Bob} -> {Alice,Carol}, identical on both sides.
        let rotation = || {
            build_signed_shared_action(
                false,
                anchor_id,
                b"v0".to_vec(),
                rotated.clone(),
                30,
                &alice_sk,
                vec![],
            )
        };

        // Receiver: applies the rotation as a delta (maybe_append + fold).
        let receiver = Store::new(Arc::new(InMemoryDB::owned()));
        bootstrap(&receiver);
        with_runtime_env(create_runtime_env(&receiver, context_id, identity), || {
            Interface::<MainStorage>::apply_action(
                rotation(),
                &ApplyContext {
                    effective_writers: Some(full_mask(genesis.clone())),
                    delta_id: Some(rotation_delta_id),
                    delta_hlc: Some(hlc(30)),
                },
            )
            .expect("receiver applies rotation");
        });

        // Originator: records the SAME rotation via the leg-4 primitive (the
        // local write left the anchor's own_hash folding the pre-rotation set).
        let originator = Store::new(Arc::new(InMemoryDB::owned()));
        bootstrap(&originator);
        with_runtime_env(
            create_runtime_env(&originator, context_id, identity),
            || {
                let changed = Interface::<MainStorage>::self_log_and_rehash_own_rotations(
                    &[rotation()],
                    rotation_delta_id,
                    hlc(30),
                )
                .expect("originator self-log + rehash");
                assert!(changed, "leg 4 must register the originator's own rotation");
            },
        );

        assert_eq!(
            own_hash(&originator),
            own_hash(&receiver),
            "originator (self_log_and_rehash) and receiver (apply_action) must land \
             the same folded anchor own_hash"
        );
    }
}
