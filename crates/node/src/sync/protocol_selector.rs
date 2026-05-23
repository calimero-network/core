//! Protocol-dispatch for the initiator side of a sync session.
//!
//! `SyncManager::handle_dag_sync` calls [`select_protocol`] against
//! local + remote handshakes to choose a sync protocol, then forwards
//! the resulting `ProtocolSelection` into this module's
//! [`ProtocolSelector::execute`], which runs the chosen protocol and
//! walks the fallback chain when one fails:
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
//!
//! Extracted from `SyncManager::handle_dag_sync` as Phase 4 of #2313.
//! The cross-protocol callbacks (`fallback_to_snapshot_sync` and
//! `request_dag_heads_and_sync`) stay on `SyncManager` and are
//! exposed through the [`ProtocolDispatch`] trait, mirroring the
//! per-call-injection pattern used by [`crate::sync::reconciler`].
//!
//! [`select_protocol`]: calimero_node_primitives::sync::select_protocol

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{ProtocolSelection, SyncProtocol, SyncProtocolExecutor};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result, WrapErr};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use super::hash_comparison_protocol::{HashComparisonConfig, HashComparisonProtocol};
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

impl ProtocolSelector {
    pub(crate) fn new(context_client: ContextClient) -> Self {
        Self { context_client }
    }

    /// Execute the chosen protocol and walk the fallback chain.
    ///
    /// Returns `Ok(Some(protocol))` with the protocol the session
    /// actually completed with (which may differ from
    /// `selection.protocol` if a fallback fired), `Ok(None)` when the
    /// selection was `SyncProtocol::None` (already converged), or
    /// `Err(_)` if every protocol in the chain failed.
    ///
    /// `local_root_hash` is included in the `None` arm's debug log
    /// so operators can correlate "no sync needed" entries with the
    /// state of the local context. `peer_root_hash` is the deref'd
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
    pub(crate) async fn execute<D: ProtocolDispatch>(
        &self,
        dispatch: &D,
        selection: ProtocolSelection,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        local_root_hash: &Hash,
        peer_root_hash: &Hash,
        stream: &mut Stream,
    ) -> Result<Option<SyncProtocol>> {
        match selection.protocol {
            SyncProtocol::None => {
                debug!(
                    %context_id,
                    %chosen_peer,
                    root_hash = %local_root_hash,
                    reason = %selection.reason,
                    "No sync needed: {}",
                    selection.reason
                );
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
                info!(
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
                        info!(
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

                        Ok(Some(SyncProtocol::HashComparison {
                            root_hash,
                            divergent_subtrees: vec![],
                        }))
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
                    remote_root_hash: **peer_root_hash,
                    max_depth,
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

/// Apply root-entity merges that HC's DFS deferred because the host can't
/// dispatch the app's typed `Mergeable::merge` (the registry it would
/// consult is populated inside WASM, separate address space). For each
/// deferred `(entity_id, incoming_bytes)` pair:
///
/// 1. Read the locally-stored bytes + metadata for `entity_id`.
/// 2. Build a [`MergeRootStateRequest`] from existing + incoming.
/// 3. Invoke `ContextClient::merge_root_state` — calls into WASM via
///    the macro-generated `__calimero_merge_root_state` export.
/// 4. Write the merged bytes back via
///    `Interface::write_pre_merged_root_state`, which updates the
///    Merkle index + storage without re-running the merge step.
///
/// Errors per-entry are logged and the next entry is attempted —
/// partial progress is preferable to dropping the whole batch on a
/// single failure. The next sync tick will re-attempt anything that
/// stays divergent.
async fn dispatch_deferred_root_merges(
    context_client: &ContextClient,
    store: &calimero_store::Store,
    context_id: ContextId,
    our_identity: PublicKey,
    deferred: &[([u8; 32], Vec<u8>)],
) {
    use calimero_storage::address::Id;
    use calimero_storage::entities::Metadata;
    use calimero_storage::env::with_runtime_env;
    use calimero_storage::index::Index;
    use calimero_storage::interface::Interface;
    use calimero_storage::merge::MergeRootStateRequest;
    use calimero_storage::store::{MainStorage, StorageAdaptor};

    // Build a runtime env so storage callbacks resolve against the
    // right context — mirrors what HC initiator does for its DFS.
    let runtime_env = calimero_node_primitives::sync::create_runtime_env(
        store,
        context_id,
        our_identity,
    );

    for (key, incoming) in deferred {
        let entity_id = Id::new(*key);

        // Read existing bytes + metadata under the runtime env so
        // storage callbacks resolve. `get_metadata` returns `None` if
        // the receiver has never seen the root entity — in that case
        // existing is empty + timestamps are 0, and the WASM-side
        // bootstrap fast-path (existing.created_at == existing.updated_at)
        // accepts incoming unconditionally.
        let read_result: eyre::Result<(Vec<u8>, Metadata)> =
            with_runtime_env(runtime_env.clone(), || {
                let meta = Index::<MainStorage>::get_index(entity_id)
                    .map_err(|e| eyre::eyre!("get_index: {e}"))?
                    .map(|idx| idx.metadata)
                    .unwrap_or_default();
                let existing = <MainStorage as StorageAdaptor>::storage_read(
                    calimero_storage::store::Key::Entry(entity_id),
                )
                .unwrap_or_default();
                Ok((existing, meta))
            });

        let (existing, existing_metadata) = match read_result {
            Ok(pair) => pair,
            Err(err) => {
                warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    %err,
                    "deferred root merge: failed to read existing root state, skipping"
                );
                continue;
            }
        };

        // Use the maximum of existing and incoming timestamps for the
        // merged result's updated_at — convention matches LWW timestamp
        // semantics and ensures the merged write strictly advances the
        // local logical clock for this entity.
        let existing_ts: u64 = (*existing_metadata.updated_at).into();
        let incoming_ts: u64 = existing_ts.max(
            // HC leaves don't carry an HLC-keyed update timestamp for
            // the root entity (the leaf carries its own metadata, but
            // that's per-leaf, not per-root-merge). Use existing_ts + 1
            // as a synthetic monotonic tick; the merge IS the write so
            // the local clock must advance.
            existing_ts.saturating_add(1),
        );

        let request = MergeRootStateRequest {
            existing,
            incoming: incoming.clone(),
            existing_created_at: existing_metadata.created_at,
            existing_ts,
            incoming_ts,
        };

        let merged = match context_client
            .merge_root_state(&context_id, &our_identity, request)
            .await
        {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    ?err,
                    "deferred root merge: WASM dispatch failed, skipping"
                );
                continue;
            }
        };

        // Write merged bytes back. Bump `updated_at` to incoming_ts so the
        // post-merge state is timestamped consistently for the next sync.
        let mut new_metadata = existing_metadata.clone();
        new_metadata.updated_at = incoming_ts.into();

        let write_result = with_runtime_env(runtime_env.clone(), || {
            Interface::<MainStorage>::write_pre_merged_root_state(
                entity_id,
                &merged,
                new_metadata,
            )
            .map_err(|e| eyre::eyre!("write_pre_merged_root_state: {e}"))
        });

        match write_result {
            Ok(_full_hash) => {
                info!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    "deferred root merge: applied"
                );
            }
            Err(err) => {
                warn!(
                    %context_id,
                    entity_id = %hex::encode(key),
                    %err,
                    "deferred root merge: failed to write merged bytes back"
                );
            }
        }

    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    // Orchestration tests for `ProtocolSelector::execute` need both a
    // mockable `ProtocolDispatch` AND a way to construct a `Stream` /
    // `StreamTransport` from a synthetic transport — neither of which
    // is cheap to wire up today (`Stream` wraps a real `libp2p::Stream`,
    // and the HashComparison / LevelWise initiators take a transport
    // that's tightly coupled to `Stream` via `StreamTransport`). The
    // `Snapshot` / `BloomFilter` / `SubtreePrefetch` / `None` /
    // `DeltaSync` arms could be tested directly with a `MockDispatch`
    // alone — those arms never touch the stream-transport surface,
    // only `dispatch.*` callbacks — but the higher-leverage HashComparison
    // and LevelWise fallback chains genuinely need a `Stream` fixture.
    //
    // Tracked in issue #2458 alongside the broader sync-test-fixture
    // work. The dispatch body moved verbatim from
    // `SyncManager::handle_dag_sync` (lines 1492-1749 pre-extraction),
    // so the existing partition-scenario integration tests
    // (`p3_dag_causal_tests`, `p5_partition_scenarios_tests`) continue
    // to exercise every fallback path end-to-end in the meantime.
}
