//! Protocol-dispatch for the initiator side of a sync session.
//!
//! `SyncManager::handle_dag_sync` decides a `ProtocolSelection` and forwards it
//! into this module's [`ProtocolSelector::execute`], which runs the chosen
//! protocol and walks the fallback chain when one fails:
//!
//! The choice is made in `SyncManager::handle_dag_sync`, which does call
//! `select_protocol` — this module only executes the result.
//!
//! `select_protocol` used to begin with a version-compatibility check that
//! could never fail. `build_remote_handshake` fabricates the peer's handshake
//! LOCALLY from the only two things that cross the wire (its root hash and dag
//! heads), so the peer's `version` field was filled from this node's own
//! `SYNC_PROTOCOL_VERSION`. The comparison was a constant against itself, run
//! on every sync. The constant is gone; #3810 tracks building the negotiation
//! CIP §2.3 actually specifies.
//!
//! - `None` → no-op, sync converged on root-hash match.
//! - `Snapshot { .. }` → `fallback_to_snapshot_sync`.
//! - `DeltaSync { .. }` → `request_dag_heads_and_sync`.
//! - `HashComparison { .. }` → run the protocol, fall back to
//!   DAG-heads sync on failure, fall back to snapshot on a further
//!   `None` result.
//! - `BloomFilter`, `SubtreePrefetch` → not implemented; fall through
//!   to snapshot.
//! - `LevelWise { .. }` → run the protocol, fall back to DAG-heads
//!   sync on failure (on a freshly-opened stream), fall back to
//!   snapshot if that also returns `None`.
//! - HashComparison or LevelWise against a peer with no known identity in
//!   a group-governed context → `DeltaSync`, before any state walk.
//!
//! Extracted from `SyncManager::handle_dag_sync` as Phase 4 of #2313.
//! The cross-protocol callbacks (`fallback_to_snapshot_sync` and
//! `request_dag_heads_and_sync`) stay on `SyncManager` and are
//! exposed through the [`ProtocolDispatch`] trait, mirroring the
//! per-call-injection pattern used by [`crate::sync::reconciler`].

use std::future::Future;

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::ExecuteError;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    InitProof, ProtocolSelection, SyncProtocol, SyncProtocolExecutor, TreeLeafData,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::interface::Interface;
use calimero_storage::merge::{MergeRootStateRequest, MergeRootStateResponse};
use calimero_storage::store::MainStorage;
use eyre::{bail, ensure, Result, WrapErr};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use super::hash_comparison_protocol::{HashComparisonConfig, HashComparisonProtocol};
use super::helpers::{apply_under_context_lock, unreadable_schema};
use super::level_sync::{LevelWiseConfig, LevelWiseProtocol};

/// Methods on `SyncManager` that the protocol-dispatch path calls
/// back into. Mirrors the [`super::reconciler::ReconcileSyncDispatch`]
/// shape: trait passed per-call, `?Send` because the callers are not
/// Send-safe internally (delta-store iterators across awaits), and
/// the selector is awaited from a single async task in the run loop.
#[async_trait(?Send)]
pub(crate) trait ProtocolDispatch {
    /// Open a fresh sync stream to `peer`. Used by the LevelWise
    /// fallback path which needs a new stream after the previous one
    /// has left the responder in a protocol-specific state.
    async fn open_stream(&self, peer: PeerId) -> Result<Stream>;

    /// Send the DAG-heads request and let the peer drive a regular
    /// delta-sync over the same stream.
    async fn request_dag_heads_and_sync(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> Result<SyncProtocol>;

    /// Pull state from the peer via the snapshot protocol. Used as the
    /// last-resort fallback when both HashComparison/LevelWise and
    /// DAG-heads sync are insufficient.
    async fn fallback_to_snapshot_sync(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        chosen_peer: PeerId,
    ) -> Result<SyncProtocol>;

    /// Build the transport-binding proof of possession for `party_id` in
    /// `context_id`, to attach to the state-read `Init`s the HashComparison /
    /// LevelWise initiators send. `None` when this node can't sign for
    /// `party_id`. See [`InitProof`].
    async fn build_init_pop(&self, context_id: ContextId, party_id: PublicKey)
        -> Option<InitProof>;
}

/// Protocol-dispatch component.
///
/// Owns the `ContextClient` (cheap to clone) for direct datastore
/// access during HashComparison / LevelWise execution. The
/// dispatch-callbacks (`fallback_to_snapshot_sync`,
/// `request_dag_heads_and_sync`, `open_stream`) are passed in per-call
/// via [`ProtocolDispatch`] so the selector can be unit-tested
/// without spinning up a `SyncManager`.
#[derive(Clone)]
pub(crate) struct ProtocolSelector {
    context_client: ContextClient,
}

/// The two root hashes a sync compares, named so they cannot be swapped.
///
/// They were adjacent `&Hash` parameters. Transposing them compiled, and the
/// consequence was silent: the local hash would be forwarded as
/// `remote_root_hash` into the HashComparison / LevelWise initiator config, so
/// the protocol would reconcile against this node's own state and conclude there
/// was nothing to fetch. Nothing downstream can tell the two apart afterwards —
/// they are the same type and both are legitimate values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RootHashPair<'a> {
    /// This node's root hash for the context.
    pub(crate) local: &'a Hash,
    /// The peer's root hash, as it reported it.
    pub(crate) peer: &'a Hash,
}

impl ProtocolSelector {
    pub(crate) fn new(context_client: ContextClient) -> Self {
        Self { context_client }
    }

    /// Where a group governs, a state walk applies authorless rows only from an
    /// attributed peer, so an unattributed one is caught up through signed deltas.
    fn route_unattributed_peer(
        &self,
        selection: ProtocolSelection,
        context_id: ContextId,
        session_peer: Option<PublicKey>,
    ) -> ProtocolSelection {
        let walks_state = matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. } | SyncProtocol::LevelWise { .. }
        );
        let ungoverned = matches!(
            calimero_governance_store::get_group_for_context(
                self.context_client.datastore(),
                &context_id
            ),
            Ok(None)
        );
        if !walks_state || session_peer.is_some() || ungoverned {
            return selection;
        }
        ProtocolSelection {
            protocol: SyncProtocol::DeltaSync {
                missing_delta_ids: vec![],
            },
            reason: "peer not attributed to an identity in a group context",
        }
    }

    /// Execute the chosen protocol and walk the fallback chain.
    ///
    /// Returns `Ok(Some(protocol))` with the protocol the session
    /// actually completed with (which may differ from
    /// `selection.protocol` if a fallback fired), `Ok(None)` when the
    /// selection was `SyncProtocol::None` (already converged), or
    /// `Err(_)` if every protocol in the chain failed.
    ///
    /// `roots.local` is included in the `None` arm's debug log
    /// so operators can correlate "no sync needed" entries with the
    /// state of the local context. `roots.peer` is the deref'd
    /// `[u8; 32]` of the peer's root — needed by `LevelWiseConfig`.
    ///
    /// ## Stream postconditions
    ///
    /// `stream` is the established sync stream. The selector borrows
    /// it for the duration of the call but does not return it; the
    /// caller does not get to know which state it ends in without
    /// reading the arms. Per-arm:
    ///
    /// - `None`: untouched.
    /// - `Snapshot`, `BloomFilter`, `SubtreePrefetch`: untouched —
    ///   `fallback_to_snapshot_sync` opens its own stream.
    /// - `DeltaSync`: passed straight to `request_dag_heads_and_sync`,
    ///   which may consume it; indeterminate on return.
    /// - `HashComparison`: passed to `StreamTransport`, consumed by
    ///   `HashComparisonProtocol::run_initiator`; the fallback path
    ///   opens a fresh stream because the responder dispatch is
    ///   one-shot per stream — once the HashComparison handler
    ///   returns the stream is dropped and can't carry a follow-up
    ///   request.
    /// - `LevelWise`: passed to `StreamTransport`, consumed by
    ///   `LevelWiseProtocol::run_initiator`; the fallback path opens
    ///   a fresh stream for the same one-shot-dispatch reason
    ///   (responder is also locked into LevelWise-specific message
    ///   types until the handler returns).
    ///
    /// Bottom line: `stream` should be treated as consumed after this
    /// call returns regardless of variant — the caller's drop runs
    /// after the function returns and closes it cleanly either way.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute<D: ProtocolDispatch>(
        &self,
        dispatch: &D,
        selection: ProtocolSelection,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        roots: RootHashPair<'_>,
        // Remote peer's attributable member identity, forwarded to the
        // HC / LevelWise initiator configs to gate authorless leaves.
        session_peer: Option<PublicKey>,
        stream: &mut Stream,
    ) -> Result<Option<SyncProtocol>> {
        let selection = self.route_unattributed_peer(selection, context_id, session_peer);
        match selection.protocol {
            SyncProtocol::None => {
                debug!(
                    %context_id,
                    %chosen_peer,
                    root_hash = %roots.local,
                    reason = %selection.reason,
                    "No sync needed: {}",
                    selection.reason
                );
                // (S2.3: the standalone rotation-log reconcile on the roots-match
                // path was removed. The rotation log is now a hashed
                // `UnorderedMap` child of its anchor, so a writer-set rotation
                // MOVES the anchor's `full_hash` and thus the Merkle root — equal
                // roots now genuinely imply equal writer sets, and any divergence
                // is reconciled by ordinary HashComparison. No separate reconcile
                // is needed.)
                Ok(None)
            }
            SyncProtocol::Snapshot { compressed, .. } => {
                info!(
                    %context_id,
                    %chosen_peer,
                    compressed,
                    reason = %selection.reason,
                    "Initiating snapshot sync"
                );
                let result = dispatch
                    .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer)
                    .await
                    .wrap_err("snapshot sync")?;
                Ok(Some(result))
            }
            SyncProtocol::DeltaSync { .. } => {
                info!(
                    %context_id,
                    %chosen_peer,
                    reason = %selection.reason,
                    "Initiating delta sync via DAG heads request"
                );
                let result = dispatch
                    .request_dag_heads_and_sync(context_id, chosen_peer, our_identity, stream)
                    .await
                    .wrap_err("delta sync")?;

                // Unlike HashComparison/LevelWise, DeltaSync does not
                // fall back to snapshot when the peer turns out to
                // have no data: `select_protocol` only picks DeltaSync
                // when the local + remote handshakes already showed
                // overlapping state, so a `None` here is a real wire-
                // level discrepancy (peer's handshake claimed data,
                // delta request found none) that should bubble up so
                // the caller picks a different peer or backs off,
                // rather than silently rolling forward into a snapshot
                // sync against a peer whose state is suspect.
                if matches!(result, SyncProtocol::None) {
                    bail!(
                        "Peer {chosen_peer} has no data for context {context_id} \
                         despite handshake indicating overlap"
                    );
                }

                Ok(Some(result))
            }
            SyncProtocol::HashComparison { root_hash, .. } => {
                debug!(
                    %context_id,
                    reason = %selection.reason,
                    "Starting HashComparison sync"
                );

                // Wrap stream in transport abstraction
                let mut transport = super::stream::StreamTransport::new(stream);

                // Get store for protocol execution
                let store = self.context_client.datastore_handle().into_inner();
                let config = HashComparisonConfig {
                    remote_root_hash: root_hash,
                    context_client: Some(self.context_client.clone()),
                    session_peer,
                    init_pop: dispatch.build_init_pop(context_id, our_identity).await,
                };

                match HashComparisonProtocol::run_initiator(
                    &mut transport,
                    &store,
                    context_id,
                    our_identity,
                    config,
                )
                .await
                {
                    Ok(stats) => {
                        debug!(
                            %context_id,
                            nodes_compared = stats.nodes_compared,
                            entities_merged = stats.entities_merged,
                            nodes_skipped = stats.nodes_skipped,
                            deferred_root_merges = stats.deferred_root_merges.len(),
                            "HashComparison sync completed successfully"
                        );

                        // Dispatch any deferred root-entity merges through
                        // the WASM module. HC's DFS can't merge root
                        // entities on the host side (the merge registry
                        // it would consult is populated inside WASM,
                        // not here), so it accumulates them in
                        // `stats.deferred_root_merges` and lets the
                        // selector finish the job using
                        // `ContextClient::merge_root_state`.
                        if !stats.deferred_root_merges.is_empty() {
                            dispatch_deferred_root_merges(
                                &self.context_client,
                                &store,
                                context_id,
                                our_identity,
                                &stats.deferred_root_merges,
                            )
                            .await;
                        }

                        // Same pass for custom-typed entries. Both dispatchers
                        // run here rather than inside the DFS because neither
                        // can call into WASM from a synchronous apply.
                        if !stats.deferred_custom_merges.is_empty() {
                            dispatch_deferred_custom_merges(
                                &self.context_client,
                                &store,
                                context_id,
                                our_identity,
                                &stats.deferred_custom_merges,
                            )
                            .await;
                        }

                        // P6.S3: the post-sync governance-divergence pull moved to
                        // a single check in the manager (`handle_dag_sync`, after
                        // `execute` returns) so it covers EVERY data backend —
                        // Snapshot / DeltaSync as well as HC / LevelWise — not just
                        // the two that compute the verdict here. The selector stays
                        // a pure data-transfer step.
                        Ok(Some(SyncProtocol::HashComparison { root_hash }))
                    }
                    Err(e) => {
                        warn!(
                            %context_id,
                            error = %e,
                            "HashComparison sync failed, falling back to DAG catchup"
                        );
                        // Fall back to DAG heads request — open a fresh
                        // stream. The HashComparison responder loop
                        // gracefully exits on any non-TreeNodeRequest /
                        // non-EntityPush message, but the responder
                        // dispatch in `internal_handle_opened_stream` is
                        // one-shot per stream: when the HashComparison
                        // handler returns, the stream is dropped. So
                        // sending a `DagHeadsRequest` on the same
                        // `stream` here would either hit a closed pipe
                        // or write into a buffer the responder will
                        // never read. A fresh stream re-enters the
                        // responder dispatch and gets routed to
                        // `handle_dag_heads_request`. Same shape as the
                        // LevelWise fallback below.
                        let mut fallback_stream = dispatch
                            .open_stream(chosen_peer)
                            .await
                            .wrap_err("open stream for hash-comparison fallback")?;
                        let result = dispatch
                            .request_dag_heads_and_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                &mut fallback_stream,
                            )
                            .await
                            .wrap_err("hash comparison fallback")?;

                        if matches!(result, SyncProtocol::None) {
                            // If DAG catchup doesn't work, try snapshot as last resort
                            info!(
                                %context_id,
                                "DAG catchup failed, falling back to snapshot sync"
                            );
                            // Drop the consumed fallback_stream before
                            // opening fresh streams in snapshot sync
                            // (fallback_stream is in indeterminate
                            // state after DAG sync exchanges).
                            drop(fallback_stream);
                            let result = dispatch
                                .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer)
                                .await
                                .wrap_err("snapshot fallback")?;
                            return Ok(Some(result));
                        }

                        Ok(Some(result))
                    }
                }
            }
            SyncProtocol::BloomFilter { .. } => {
                warn!(
                    %context_id,
                    reason = %selection.reason,
                    "BloomFilter not yet implemented, falling back to snapshot"
                );
                let result = dispatch
                    .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer)
                    .await
                    .wrap_err("bloom filter fallback")?;
                Ok(Some(result))
            }
            SyncProtocol::SubtreePrefetch { .. } => {
                warn!(
                    %context_id,
                    reason = %selection.reason,
                    "SubtreePrefetch not yet implemented, falling back to snapshot"
                );
                let result = dispatch
                    .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer)
                    .await
                    .wrap_err("subtree prefetch fallback")?;
                Ok(Some(result))
            }
            SyncProtocol::LevelWise { max_depth } => {
                info!(
                    %context_id,
                    max_depth,
                    reason = %selection.reason,
                    "Starting LevelWise sync"
                );

                // Wrap stream in transport abstraction
                let mut transport = super::stream::StreamTransport::new(stream);

                // Get store for protocol execution
                let store = self.context_client.datastore_handle().into_inner();
                let config = LevelWiseConfig {
                    remote_root_hash: **roots.peer,
                    max_depth,
                    context_client: Some(self.context_client.clone()),
                    session_peer,
                    init_pop: dispatch.build_init_pop(context_id, our_identity).await,
                };

                match LevelWiseProtocol::run_initiator(
                    &mut transport,
                    &store,
                    context_id,
                    our_identity,
                    config,
                )
                .await
                {
                    Ok(stats) => {
                        info!(
                            %context_id,
                            levels_synced = stats.levels_synced,
                            nodes_compared = stats.nodes_compared,
                            entities_merged = stats.entities_merged,
                            nodes_skipped = stats.nodes_skipped,
                            deferred_root_merges = stats.deferred_root_merges.len(),
                            "LevelWise sync completed successfully"
                        );

                        // Same deferred-root-merge dispatch as HC; the
                        // BFS encounters root-entity leaves it can't
                        // merge on the host.
                        if !stats.deferred_root_merges.is_empty() {
                            dispatch_deferred_root_merges(
                                &self.context_client,
                                &store,
                                context_id,
                                our_identity,
                                &stats.deferred_root_merges,
                            )
                            .await;
                        }

                        // Same pass for custom-typed entries. Both dispatchers
                        // run here rather than inside the DFS because neither
                        // can call into WASM from a synchronous apply.
                        if !stats.deferred_custom_merges.is_empty() {
                            dispatch_deferred_custom_merges(
                                &self.context_client,
                                &store,
                                context_id,
                                our_identity,
                                &stats.deferred_custom_merges,
                            )
                            .await;
                        }

                        // P6.S3: post-sync governance pull centralised in the
                        // manager (covers all data backends); see the HashComparison
                        // arm above.
                        Ok(Some(SyncProtocol::LevelWise { max_depth }))
                    }
                    Err(e) => {
                        warn!(
                            %context_id,
                            error = %e,
                            "LevelWise sync failed, falling back to DAG catchup"
                        );
                        // Fall back to DAG heads request - open a new stream since the
                        // LevelWise protocol may have left the peer's responder in a
                        // state where it expects LevelWiseRequest messages, not
                        // DagHeadsRequest.
                        let mut fallback_stream = dispatch
                            .open_stream(chosen_peer)
                            .await
                            .wrap_err("open stream for level-wise fallback")?;
                        let result = dispatch
                            .request_dag_heads_and_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                &mut fallback_stream,
                            )
                            .await
                            .wrap_err("level-wise fallback")?;

                        if matches!(result, SyncProtocol::None) {
                            // If DAG catchup doesn't work, try snapshot as last resort
                            info!(
                                %context_id,
                                "DAG catchup insufficient, attempting snapshot"
                            );
                            // Drop the consumed fallback_stream before opening fresh
                            // streams in snapshot sync (fallback_stream is in
                            // indeterminate state after DAG sync exchanges).
                            drop(fallback_stream);
                            let snapshot_result = dispatch
                                .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer)
                                .await
                                .wrap_err("level-wise snapshot fallback")?;
                            return Ok(Some(snapshot_result));
                        }
                        Ok(Some(result))
                    }
                }
            }
        }
    }
}

/// Apply the custom-typed ENTRY merges that sync deferred, for the same reason
/// [`dispatch_deferred_root_merges`] exists: the apply is synchronous and inside
/// the storage env, so it cannot reach a rule that lives in the app's module.
///
/// Doing it here — after the session, with nothing nested — is what makes a
/// second WASM instance unnecessary. Per entry:
///
/// 1. Read the locally-stored entry bytes + metadata.
/// 2. Send both sides plus the entry's `CustomTypeId` into
///    `__calimero_merge_custom`, which resolves the id against the registry the
///    module populated at load.
/// 3. Write the merged bytes back with `write_pre_merged_root_state`, which
///    updates storage and the Merkle index without re-running the merge.
///
/// An entry with no local bytes is SKIPPED rather than accepted: unlike a root
/// there is no bootstrap case to serve, and a merge of one side is not a merge.
/// The plain apply path stores such an entry on its own.
///
/// Failures are per-entry and logged. Nothing falls back to LWW — that would
/// resolve a conflict the app declared itself responsible for, and resolve it
/// differently depending on arrival order. The entity stays divergent until the
/// next round, which is recoverable; a silently wrong value is not.
pub(crate) async fn dispatch_deferred_custom_merges(
    context_client: &ContextClient,
    store: &calimero_store::Store,
    context_id: ContextId,
    our_identity: PublicKey,
    deferred: &[(
        [u8; 32],
        calimero_primitives::crdt::CustomTypeId,
        Vec<u8>,
        u64,
    )],
) {
    use calimero_storage::address::Id;

    let Ok(account) = calimero_governance_store::account_for_context(store, &context_id) else {
        tracing::warn!(
            %context_id,
            "cannot resolve this node's account for the context; skipping the \
             deferred custom-merge pass (retried on the next sync tick)"
        );
        return;
    };
    let runtime_env = calimero_node_primitives::sync::create_runtime_env(
        store,
        context_id,
        our_identity,
        account,
    );

    for (key, type_id, incoming, incoming_hlc_ts) in deferred {
        let entity_id = Id::new(*key);
        let request =
            apply_under_context_lock(Some(context_client), context_id, &runtime_env, || {
                Interface::<MainStorage>::custom_entry_merge_request(
                    entity_id,
                    *type_id,
                    incoming.clone(),
                    *incoming_hlc_ts,
                )
            })
            .await;
        let (request, existing_metadata) = match request {
            Ok(Some(found)) => found,
            Ok(None) => {
                tracing::debug!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    "deferred custom merge: nothing stored locally, leaving it to the \
                     plain apply path"
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    %err,
                    "deferred custom merge: refused or unreadable, skipping"
                );
                continue;
            }
        };

        let merged = match context_client
            .merge_custom(&context_id, &our_identity, request.clone())
            .await
        {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    ?err,
                    "deferred custom merge: WASM dispatch failed, leaving the entity \
                     divergent for the next round"
                );
                continue;
            }
        };

        let write_result =
            apply_under_context_lock(Some(context_client), context_id, &runtime_env, || {
                Interface::<MainStorage>::write_custom_entry_merge(
                    entity_id,
                    &request,
                    &existing_metadata,
                    &merged,
                    *incoming_hlc_ts,
                )
            })
            .await;

        match write_result {
            Ok(Some(_full_hash)) => {
                tracing::info!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    "deferred custom merge: applied"
                );
            }
            Ok(None) => {
                tracing::debug!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    "deferred custom merge: the stored entry moved during the merge; \
                     the next round merges it again"
                );
            }
            Err(err) => {
                tracing::warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    %err,
                    "deferred custom merge: failed to write merged bytes back"
                );
            }
        }
    }
}

/// Merges the app-state entries sync deferred, after the session, since the apply
/// cannot reach the app's module; a failed entry is retried on the next tick.
pub(crate) async fn dispatch_deferred_root_merges(
    context_client: &ContextClient,
    store: &calimero_store::Store,
    context_id: ContextId,
    our_identity: PublicKey,
    deferred: &[TreeLeafData],
) {
    // This helper returns `()`; an unresolvable account cannot be propagated, and
    // it also cannot be invented — so log and skip the deferred batch rather than
    // gate it on the wrong principal. The next sync tick retries.
    let Ok(account) = calimero_governance_store::account_for_context(store, &context_id) else {
        tracing::warn!(
            %context_id,
            "cannot resolve this node's account for the context; skipping the \
             deferred-leaf pass (retried on the next sync tick)"
        );
        return;
    };
    let runtime_env = calimero_node_primitives::sync::create_runtime_env(
        store,
        context_id,
        our_identity,
        account,
    );
    let loaded = calimero_context::hlc_fence::loaded_reader_bytecode_id(store, &context_id);

    for leaf in deferred {
        let entity_id = hex::encode(leaf.key);
        // The module merging the entry must read the schema it was written under.
        match &loaded {
            Err(err) => {
                warn!(
                    %context_id,
                    %entity_id,
                    %err,
                    "deferred root merge: loaded reader unknown, skipping"
                );
                continue;
            }
            Ok(Some(loaded)) if unreadable_schema(leaf, *loaded).is_some() => {
                debug!(
                    %context_id,
                    %entity_id,
                    "deferred root merge: written under another schema, skipping"
                );
                continue;
            }
            _ => {}
        }
        let merged = merge_deferred_root(
            Some(context_client),
            context_id,
            &runtime_env,
            leaf,
            |request| context_client.merge_root_state(&context_id, &our_identity, request),
        )
        .await;
        match merged {
            Ok(()) => info!(%context_id, %entity_id, "deferred root merge: applied"),
            Err(err) => warn!(%context_id, %entity_id, %err, "deferred root merge: not applied"),
        }
    }
}

/// Merges one deferred app-state entry by the rule a delta's write of it takes:
/// the stamp bound, then the app's merge, written back through storage.
pub async fn merge_deferred_root<F, Fut>(
    context_client: Option<&ContextClient>,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    leaf: &TreeLeafData,
    merge: F,
) -> eyre::Result<()>
where
    F: FnOnce(MergeRootStateRequest) -> Fut,
    Fut: Future<Output = Result<MergeRootStateResponse, ExecuteError>>,
{
    let request = apply_under_context_lock(context_client, context_id, runtime_env, || {
        Interface::<MainStorage>::root_entry_merge_request(
            leaf.value.clone(),
            leaf.metadata.hlc_timestamp,
        )
    })
    .await?;
    let merged = match merge(request.clone()).await? {
        MergeRootStateResponse::Ok(merged) => Some(merged),
        MergeRootStateResponse::Err(_) => None,
        MergeRootStateResponse::Refused(reason) => bail!("the app refused the entry: {reason}"),
    };
    let written = apply_under_context_lock(context_client, context_id, runtime_env, || {
        Interface::<MainStorage>::write_root_entry_merge(
            &request,
            merged.as_deref(),
            leaf.metadata.created_at,
        )
    })
    .await?;
    ensure!(
        written.is_some(),
        "the stored entry moved during the merge; the next round merges it again"
    );
    Ok(())
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    // `execute` arms are driven through `Stream::test_pair`; the partition-scenario
    // integration tests cover the full fallback chains end to end.
    use super::*;
    use super::{ProtocolDispatch, ProtocolSelector, RootHashPair};
    use crate::sync::helpers::apply_leaf_with_crdt_merge;
    use crate::test_node_harness::boot_test_node;
    use async_trait::async_trait;
    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::test_fixtures::test_meta;
    use calimero_governance_store::{register_context_in_group, MetaRepository};
    use calimero_network_primitives::stream::Stream;
    use calimero_node_primitives::sync::{create_runtime_env, LeafMetadata};
    use calimero_node_primitives::sync::{InitProof, ProtocolSelection, SyncProtocol};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::crdt::CrdtType;
    use calimero_primitives::hash::Hash;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::address::Id;
    use calimero_storage::collections::ROOT_ENTRY_ID;
    use calimero_storage::entities::Metadata;
    use calimero_storage::env::{time_now, with_runtime_env, RuntimeEnv};
    use calimero_storage::index::Index;
    use calimero_storage::store::{Key, StorageAdaptor};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use eyre::Result;
    use libp2p::PeerId;
    use serial_test::serial;
    /// Answers a DAG-heads catch-up and counts the requests; opens no other stream.
    use std::cell::Cell;
    use std::sync::Arc;

    const STORED: &[u8] = b"stored entry";

    fn context() -> ContextId {
        ContextId::from([0xCB; 32])
    }

    /// A context holding the app-state entry `STORED`, written at `now - 1 s`.
    fn context_with_an_app_state_entry() -> (ContextId, RuntimeEnv) {
        let context_id = context();
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let env = create_runtime_env(
            &store,
            context_id,
            PublicKey::from([0; 32]),
            AccountId::from([0xAC; 32]),
        );
        with_runtime_env(env.clone(), || {
            let at = time_now() - 1_000_000_000;
            Interface::<MainStorage>::save_root_entry(STORED.to_vec(), Metadata::new(at, at))
                .expect("seed the app-state entry");
        });
        (context_id, env)
    }

    fn leaf(id: Id, value: &[u8], at: u64) -> TreeLeafData {
        TreeLeafData::new(
            *id.as_bytes(),
            value.to_vec(),
            LeafMetadata::new(CrdtType::opaque_leaf(), at, [0; 32]),
        )
    }

    fn stored(env: &RuntimeEnv, id: Id) -> (Vec<u8>, u64) {
        with_runtime_env(env.clone(), || {
            let bytes = MainStorage::storage_read(Key::Entry(id)).expect("stored bytes");
            let index = Index::<MainStorage>::get_index(id)
                .expect("index")
                .expect("stored index");
            (bytes, *index.metadata.updated_at)
        })
    }

    async fn merge_answering(
        env: &RuntimeEnv,
        leaf: &TreeLeafData,
        answer: MergeRootStateResponse,
    ) -> eyre::Result<()> {
        merge_deferred_root(None, context(), env, leaf, |_| async { Ok(answer) }).await
    }

    #[test]
    fn a_repair_leaf_for_the_root_collection_moves_nothing_unless_it_is_the_shell() {
        let (context_id, env) = context_with_an_app_state_entry();
        let root = Id::new(*context_id.as_ref());
        let before = stored(&env, root);

        let far_ahead = with_runtime_env(env.clone(), || {
            apply_leaf_with_crdt_merge(context_id, &leaf(root, &before.0, u64::MAX))
        });
        let undecodable = with_runtime_env(env.clone(), || {
            apply_leaf_with_crdt_merge(context_id, &leaf(root, &[0xFF; 3], time_now()))
        });

        assert!(
            far_ahead.is_ok(),
            "a restated shell is skipped, got {far_ahead:?}"
        );
        assert!(
            undecodable.is_err(),
            "bytes that are not the shell are refused"
        );
        assert_eq!(stored(&env, root), before, "the root must not move");
    }

    #[tokio::test]
    async fn a_deferred_app_state_entry_stamped_far_ahead_is_refused_before_the_merge() {
        let (_, env) = context_with_an_app_state_entry();
        let asked = Cell::new(false);

        let merged = merge_deferred_root(
            None,
            context(),
            &env,
            &leaf(ROOT_ENTRY_ID, b"peer", u64::MAX),
            |_| {
                asked.set(true);
                async { Ok(MergeRootStateResponse::Ok(b"merged".to_vec())) }
            },
        )
        .await;

        assert!(merged.is_err(), "a far-future stamp must be refused");
        assert!(!asked.get(), "the app must not be asked to merge it");
        assert_eq!(stored(&env, ROOT_ENTRY_ID).0, STORED);
    }

    #[tokio::test]
    async fn a_deferred_app_state_entry_the_app_refuses_is_not_written() {
        let (_, env) = context_with_an_app_state_entry();

        let merged = merge_answering(
            &env,
            &leaf(ROOT_ENTRY_ID, &[0xFF; 3], time_now()),
            MergeRootStateResponse::Refused("not the app's state".to_owned()),
        )
        .await;

        assert!(merged.is_err());
        assert_eq!(stored(&env, ROOT_ENTRY_ID).0, STORED);
    }

    #[tokio::test]
    async fn a_deferred_app_state_entry_is_written_as_the_app_merged_it() {
        let (_, env) = context_with_an_app_state_entry();
        let (_, stored_at) = stored(&env, ROOT_ENTRY_ID);
        let older = stored_at - 1;

        merge_answering(
            &env,
            &leaf(ROOT_ENTRY_ID, b"peer", older),
            MergeRootStateResponse::Ok(b"merged".to_vec()),
        )
        .await
        .expect("a merged entry is written");

        assert_eq!(
            stored(&env, ROOT_ENTRY_ID),
            (b"merged".to_vec(), stored_at),
            "the merge is written, as new as the newer write"
        );
    }

    #[tokio::test]
    async fn a_module_without_the_entry_merge_resolves_it_by_last_writer_wins() {
        let (_, env) = context_with_an_app_state_entry();
        let (_, stored_at) = stored(&env, ROOT_ENTRY_ID);
        let no_merge = || MergeRootStateResponse::Err("no entry merge".to_owned());

        merge_answering(
            &env,
            &leaf(ROOT_ENTRY_ID, b"older", stored_at - 1),
            no_merge(),
        )
        .await
        .expect("an older entry is a no-op");
        assert_eq!(
            stored(&env, ROOT_ENTRY_ID).0,
            STORED,
            "the older write loses"
        );

        let newer = time_now();
        merge_answering(&env, &leaf(ROOT_ENTRY_ID, b"newer", newer), no_merge())
            .await
            .expect("a newer entry is written");
        assert_eq!(stored(&env, ROOT_ENTRY_ID), (b"newer".to_vec(), newer));
    }

    #[derive(Default)]
    struct DeltaCatchUp {
        requested: Cell<u32>,
    }

    #[async_trait(?Send)]
    impl ProtocolDispatch for DeltaCatchUp {
        async fn open_stream(&self, _peer: PeerId) -> Result<Stream> {
            eyre::bail!("no further streams in this test")
        }

        async fn request_dag_heads_and_sync(
            &self,
            _context_id: ContextId,
            _chosen_peer: PeerId,
            _our_identity: PublicKey,
            _stream: &mut Stream,
        ) -> Result<SyncProtocol> {
            self.requested.set(self.requested.get() + 1);
            Ok(SyncProtocol::DeltaSync {
                missing_delta_ids: vec![],
            })
        }

        async fn fallback_to_snapshot_sync(
            &self,
            _context_id: ContextId,
            _our_identity: PublicKey,
            _chosen_peer: PeerId,
        ) -> Result<SyncProtocol> {
            eyre::bail!("no snapshot in this test")
        }

        async fn build_init_pop(
            &self,
            _context_id: ContextId,
            _party_id: PublicKey,
        ) -> Option<InitProof> {
            None
        }
    }

    /// A state walk against an unattributed peer in a group context becomes a delta
    /// catch-up; an attributed peer, or a context with no group, keeps the walk.
    #[tokio::test]
    #[serial(boot_test_node)]
    async fn an_unattributed_peer_in_a_group_context_is_caught_up_through_deltas() {
        let node = boot_test_node().await;
        let grouped = ContextId::from([0xC1; 32]);
        let group = ContextGroupId::from([0xC2; 32]);
        MetaRepository::new(&node.store)
            .save(&group, &test_meta())
            .unwrap();
        register_context_in_group(&node.store, &group, &grouped).unwrap();
        let selector = ProtocolSelector::new(node.context_client.clone());
        let root = Hash::from([7u8; 32]);

        let delta_requests =
            |protocol: SyncProtocol, context_id: ContextId, session_peer: Option<PublicKey>| {
                let selector = selector.clone();
                async move {
                    let dispatch = DeltaCatchUp::default();
                    let (mut stream, peer_end) = Stream::test_pair();
                    drop(peer_end);
                    let _outcome = selector
                        .execute(
                            &dispatch,
                            ProtocolSelection {
                                protocol,
                                reason: "test",
                            },
                            context_id,
                            PeerId::random(),
                            PublicKey::from([1u8; 32]),
                            RootHashPair {
                                local: &root,
                                peer: &root,
                            },
                            session_peer,
                            &mut stream,
                        )
                        .await;
                    dispatch.requested.get()
                }
            };

        for walk in [
            SyncProtocol::HashComparison { root_hash: [7; 32] },
            SyncProtocol::LevelWise { max_depth: 3 },
        ] {
            assert_eq!(
                delta_requests(walk.clone(), grouped, None).await,
                1,
                "{walk:?} against an unattributed peer in a group context"
            );
            assert_eq!(
                delta_requests(walk.clone(), grouped, Some(PublicKey::from([2u8; 32]))).await,
                0,
                "control: {walk:?} against an attributed peer"
            );
            assert_eq!(
                delta_requests(walk.clone(), ContextId::from([0xC3; 32]), None).await,
                0,
                "control: {walk:?} in a context no group governs"
            );
        }
    }
}
