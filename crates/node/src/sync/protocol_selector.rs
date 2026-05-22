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
    ///   `HashComparisonProtocol::run_initiator`; the responder's
    ///   state is well-defined on success (idle) and indeterminate on
    ///   failure (see the HashComparison-fallback note below).
    /// - `LevelWise`: passed to `StreamTransport`, consumed by
    ///   `LevelWiseProtocol::run_initiator`; the fallback path opens
    ///   a fresh stream because the responder may be in a
    ///   LevelWise-specific state.
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
                            "HashComparison sync completed successfully"
                        );
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
                        // Fall back to DAG heads request
                        let result = dispatch
                            .request_dag_heads_and_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                            )
                            .await
                            .wrap_err("hash comparison fallback")?;

                        if matches!(result, SyncProtocol::None) {
                            // If DAG catchup doesn't work, try snapshot as last resort
                            info!(
                                %context_id,
                                "DAG catchup failed, falling back to snapshot sync"
                            );
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
                            "LevelWise sync completed successfully"
                        );
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
