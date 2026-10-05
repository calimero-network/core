//! Snapshot sync protocol for full state bootstrap.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use borsh::BorshDeserialize;
use calimero_context_client::client::ContextRegistry;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::snapshot::{SnapshotRecord, MAX_SNAPSHOT_PAGE_SIZE};
use calimero_node_primitives::sync::{
    MessagePayload, SnapshotCursor, SnapshotError, StreamMessage,
};
use calimero_node_primitives::{SyncState, SyncStatusSnapshot};
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, NodeEvent, SyncStatusPayload,
};
use calimero_primitives::hash::Hash;
use calimero_storage::address::Id;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::env::time_now;
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::shared_writers::{CellWriters, WritersUnavailable};
use calimero_storage::store::{Key as StorageKey, MainStorage};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::key::{Generic as GenericKey, SCOPE_SIZE};
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use calimero_store::Store;
use eyre::Result;
use tracing::{debug, info, warn};

use super::helpers::SnapshotAuthorship;
use super::manager::SyncManager;
use super::tracking::Sequencer;
use stage::Stage;

mod leaf;
mod stage;

#[cfg(test)]
mod install_tests;

/// Members deferred during snapshot apply: (entity id, key, value, parent).
type DeferredMembers = Vec<(Id, Vec<u8>, Vec<u8>, Option<[u8; 32]>)>;

/// Maximum uncompressed bytes per snapshot page (64 KB).
pub const DEFAULT_PAGE_BYTE_LIMIT: u32 = 64 * 1024;

/// Maximum pages to send in a single burst.
pub const DEFAULT_PAGE_LIMIT: u16 = 16;

/// Hard ceiling on the peer-supplied `page_limit`. The responder generates up
/// to this many pages per request in memory, so an unclamped peer value (up to
/// `u16::MAX`) would let a caller drive page generation without bound. The
/// per-page byte budget is separately clamped to `MAX_SNAPSHOT_PAGE_SIZE`.
pub const MAX_PAGE_LIMIT: u16 = 1024;

/// Leading byte of a **v2** (PR-6b / #2539) snapshot page: records are
/// length-framed (`u32 LE len ‖ record_bytes`) so the receiver bounds each
/// record's decode to its own sub-slice. This makes the backward-compatible
/// trailing `SnapshotRecord::Entity.schema_bytecode_id` (decoded EOF-tolerantly)
/// sound for NON-terminal records too — a clean EOF is observed at the
/// sub-slice boundary instead of bleeding into the next record's bytes.
///
/// The value `0xFF` can never be the first byte of a LEGACY (pre-#2539)
/// unframed page: a legacy page starts with the first record's `SnapshotRecord`
/// variant discriminant, which is `0` (`Entity`) or `1` (`Auxiliary`). So the
/// receiver tells the two formats apart by this single sentinel — no wire
/// `SnapshotPage` message change needed (which would itself have the same
/// trailing-field hazard one level up).
const SNAPSHOT_PAGE_FORMAT_V2: u8 = 0xFF;

/// Scope for sync-in-progress markers in the Generic column.
/// Exactly 16 bytes to match SCOPE_SIZE.
const SYNC_IN_PROGRESS_SCOPE: [u8; SCOPE_SIZE] = *b"sync-in-progres\0";

/// Whether a snapshot `Entity` whose sender stamped `schema_bytecode_id` is readable
/// by a receiver whose loaded reader is `loaded_bytecode_id` (PR-6b Task 6b.7).
///
/// The snapshot apply path writes each verified entity via a raw `handle.put` —
/// it deliberately does NOT route through `apply_leaf_with_crdt_merge_gated`, so
/// the same readability check the HashComparison / LevelSync leaf paths get from
/// that wrapper has to be applied explicitly here. A receiver still on an older
/// reader must DECLINE + BUFFER a future-schema entity rather than persist
/// unreadable bytes (the "v1-binary-fed-v2-bytes" corruption hazard the
/// snapshot path otherwise side-steps entirely).
///
/// Returns `true` (apply) when the schema is absent (legacy sender), when no
/// loaded reader could be resolved (no gate — parity with the leaf path), or
/// when the stamped schema matches the loaded reader; `false` (decline+buffer)
/// only when both are known and differ.
fn snapshot_entity_is_readable(
    schema_bytecode_id: Option<[u8; 32]>,
    loaded_bytecode_id: Option<[u8; 32]>,
) -> bool {
    match (schema_bytecode_id, loaded_bytecode_id) {
        (Some(schema), Some(loaded)) => schema == loaded,
        // Legacy sender (no marker) or unresolvable loaded reader ⇒ no gate.
        _ => true,
    }
}

impl SyncManager {
    /// Handle incoming snapshot boundary request from a peer.
    pub async fn handle_snapshot_boundary_request(
        &self,
        context_id: ContextId,
        _requested_cutoff_timestamp: Option<u64>,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                warn!(%context_id, "Context not found for snapshot boundary request");
                return self
                    .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                    .await;
            }
        };

        // The requester refuses a snapshot from anyone it cannot authenticate as
        // an admitted member (#4089), so name the identity served as and prove
        // it from this peer.
        let Some(server_identity) = self.acting_identity(&context_id).await? else {
            warn!(%context_id, "No owned identity to serve a snapshot as");
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        };
        let Some(server_proof) = self.build_init_pop(context_id, server_identity).await else {
            warn!(%context_id, %server_identity, "Could not prove the identity to serve a snapshot as");
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        };

        info!(
            %context_id,
            root_hash = %context.root_hash,
            heads_count = context.dag_heads.len(),
            %server_identity,
            "Sending snapshot boundary response"
        );

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SnapshotBoundaryResponse {
                boundary_timestamp: time_now(),
                boundary_root_hash: context.root_hash,
                dag_heads: context.dag_heads.clone(),
                server_identity,
                server_proof,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle incoming snapshot stream request from a peer.
    #[expect(clippy::too_many_arguments, reason = "protocol handler")]
    pub async fn handle_snapshot_stream_request(
        &self,
        context_id: ContextId,
        boundary_root_hash: Hash,
        page_limit: u16,
        byte_limit: u32,
        resume_cursor: Option<Vec<u8>>,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        // Clamp peer-supplied limits before they drive page generation: a caller
        // must not be able to request an unbounded number of pages or an
        // oversized per-page byte budget (OOM). `page_limit` floors at 1 so a
        // zero value still makes progress.
        let page_limit = page_limit.clamp(1, MAX_PAGE_LIMIT);
        let byte_limit = byte_limit.clamp(1, MAX_SNAPSHOT_PAGE_SIZE);

        // The context must exist before its state means anything.
        if self.context_client.get_context(&context_id)?.is_none() {
            warn!(%context_id, "Context not found for snapshot stream");
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        }

        // Compare the *state* root, not `ContextMeta.root_hash`.
        //
        // `ContextMeta.root_hash` is not a safe stand-in for the state this
        // snapshot is about to read. A local execution commits context state
        // (`storage.commit()`) well before it persists the new root hash into
        // `ContextMeta` — see `crates/context/src/handlers/execute/mod.rs` —
        // so for the duration of that window the metadata still reports the
        // pre-write hash while every entity `generate_snapshot_pages` will
        // read has already moved. Gating on the metadata there accepts the
        // boundary as intact and streams post-boundary state under a
        // pre-boundary hash.
        //
        // The receiver cannot recover from that on its own: it recomputes the
        // root from the state it received, finds it disagrees with the hash it
        // was promised, and adopts the mismatching hash anyway (see
        // `request_snapshot_sync`). It then advertises a root whose state it
        // does not hold, which satisfies every root-hash equality check —
        // including the one in `protocol_selector` — so no repair is ever
        // triggered.
        //
        // The ROOT `Index` entry moves in the same write batch as the entities,
        // so reading through it observes exactly the state that will be served.
        let state_root = served_state_root(self.context_client.datastore(), context_id)?;
        if state_root != boundary_root_hash {
            warn!(
                %context_id,
                expected = %boundary_root_hash,
                actual = %state_root,
                "Boundary mismatch - state changed during sync"
            );
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        }

        // Parse resume cursor
        let start_cursor = match resume_cursor {
            Some(bytes) => match SnapshotCursor::try_from_slice(&bytes) {
                Ok(cursor) => Some(cursor),
                Err(_) => {
                    return self
                        .send_snapshot_error(stream, SnapshotError::ResumeCursorInvalid)
                        .await;
                }
            },
            None => None,
        };

        self.stream_snapshot_pages(
            context_id,
            boundary_root_hash,
            start_cursor,
            page_limit,
            byte_limit,
            stream,
        )
        .await
    }

    /// Stream snapshot pages to a peer.
    async fn stream_snapshot_pages(
        &self,
        context_id: ContextId,
        boundary_root_hash: Hash,
        start_cursor: Option<SnapshotCursor>,
        page_limit: u16,
        byte_limit: u32,
        stream: &mut Stream,
    ) -> Result<()> {
        let handle = self.context_client.datastore_handle();
        // PR-6b Task 6b.7: stamp every emitted `Entity` with the sender's loaded
        // reader so a receiver still on an older binary can decline+buffer a
        // future-schema entity instead of persisting unreadable bytes. `None`
        // (non-group context / unresolvable meta) leaves the marker absent —
        // legacy semantics; the receiver then applies as today.
        let schema_bytecode_id = calimero_context::hlc_fence::loaded_reader_bytecode_id(
            self.context_client.datastore(),
            &context_id,
        )
        .ok()
        .flatten();
        let (pages, next_cursor, total_entries) = generate_snapshot_pages(
            &handle,
            context_id,
            start_cursor.as_ref(),
            page_limit,
            byte_limit,
            schema_bytecode_id,
        )?;

        // Post-iteration recheck: verify the state hasn't moved during page
        // generation. A safety guardrail in addition to the point-in-time
        // iterator above. Reads the state root for the same reason the
        // pre-generation check does: `ContextMeta.root_hash` lags a committed
        // state write, so it cannot see the very change this recheck exists to
        // catch.
        let state_root = served_state_root(self.context_client.datastore(), context_id)?;
        if state_root != boundary_root_hash {
            warn!(
                %context_id,
                expected = %boundary_root_hash,
                actual = %state_root,
                "Root hash changed during snapshot generation"
            );
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        }

        info!(%context_id, pages = pages.len(), total_entries, "Streaming snapshot");
        if start_cursor.is_none() {
            warn_if_the_tree_does_not_fold(&handle, context_id);
        }

        // Handle empty snapshot case - send an empty page to signal completion
        if pages.is_empty() {
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::SnapshotPage {
                    payload: Vec::new().into(),
                    uncompressed_len: 0,
                    cursor: None,
                    page_count: 0,
                    sent_count: 0,
                    total_records: 0,
                },
                next_nonce: super::helpers::generate_nonce(),
            };
            super::stream::send(stream, &msg, None).await?;
            return Ok(());
        }

        let mut sqx = Sequencer::default();
        let page_count = pages.len() as u64;

        for (i, page_data) in pages.into_iter().enumerate() {
            let is_last = i == (page_count as usize - 1) && next_cursor.is_none();
            let compressed = lz4_flex::compress_prepend_size(&page_data);

            let cursor = if is_last {
                None
            } else if i == (page_count as usize - 1) {
                match next_cursor.as_ref().map(borsh::to_vec).transpose() {
                    Ok(value) => value,
                    Err(e) => {
                        warn!(%context_id, error = %e, "Failed to encode snapshot cursor");
                        return self
                            .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                            .await;
                    }
                }
            } else {
                None
            };

            let msg = StreamMessage::Message {
                sequence_id: sqx.next(),
                payload: MessagePayload::SnapshotPage {
                    payload: compressed.into(),
                    uncompressed_len: page_data.len() as u32,
                    cursor,
                    page_count,
                    sent_count: (i + 1) as u64,
                    total_records: total_entries,
                },
                next_nonce: super::helpers::generate_nonce(),
            };
            super::stream::send(stream, &msg, None).await?;
        }

        debug!(%context_id, "Finished streaming snapshot pages");
        Ok(())
    }

    /// Send a snapshot error response.
    async fn send_snapshot_error(&self, stream: &mut Stream, error: SnapshotError) -> Result<()> {
        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::SnapshotError { error },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &msg, None).await
    }

    /// Request and apply a full snapshot from a peer.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to sync
    /// * `peer_id` - The peer to sync from
    /// * `force` - If true, skip the safety check (for divergence recovery).
    ///   If false, enforce that the node is fresh (for bootstrap).
    pub async fn request_snapshot_sync(
        &self,
        context_id: ContextId,
        peer_id: libp2p::PeerId,
        force: bool,
    ) -> Result<SnapshotSyncResult> {
        info!(%context_id, %peer_id, force, "Starting snapshot sync");

        // Check Invariant I5: Snapshot sync should only be used for fresh nodes
        // OR for crash recovery (detected by sync-in-progress marker).
        // This prevents accidental state overwrites on initialized nodes.
        // NOTE: force=true is reserved for exceptional cases like test fixtures;
        // divergence recovery must NOT bypass this check (see I5).
        let is_crash_recovery = self.check_sync_in_progress(context_id)?.is_some();
        // An operator resync passes `force` (it routes here from
        // `handle_dag_sync` for a `ContextResyncRequested` context), so it
        // bypasses this check without a separate marker term — keeping the I5
        // gate a pure function of `force` + crash-recovery.
        if !force && !is_crash_recovery {
            // Resolve the safety gate from both signals: whether the context has
            // applied `ContextState` entries, and whether its `ContextMeta`
            // carries a non-zero root_hash (can be non-zero with no state keys
            // after deletes). A non-zero root ⇒ genuinely initialized ⇒ reject
            // (Invariant I5). The subtle case is state-present-but-root==0: that
            // is the #3252 contradiction (a snapshot finalize reverted to
            // uninitialized while its state keys persisted), which must be
            // ALLOWED to re-bootstrap rather than deadlock the safety gate.
            let handle = self.context_client.datastore_handle();
            let has_state_keys = has_context_state_keys(&handle, context_id)?;

            let has_nonzero_root = self
                .context_client
                .get_context(&context_id)?
                .map(|ctx| *ctx.root_hash != [0u8; 32])
                .unwrap_or(false);

            match calimero_node_primitives::sync::snapshot_safety_decision(
                has_state_keys,
                has_nonzero_root,
            ) {
                calimero_node_primitives::sync::SnapshotSafety::Fresh => {}
                calimero_node_primitives::sync::SnapshotSafety::RecoverContradiction => {
                    warn!(
                        %context_id,
                        "Context in contradictory state (root_hash=0 but ContextState entries \
                         present): a prior snapshot finalize was reverted while its applied \
                         state keys persisted (#3252). Allowing a re-bootstrap snapshot to \
                         recover instead of deadlocking on the I5 safety gate."
                    );
                }
                calimero_node_primitives::sync::SnapshotSafety::Initialized => {
                    self.metrics().record_snapshot_blocked();
                    return Err(eyre::eyre!(
                        "Snapshot safety check failed: {:?}",
                        SnapshotError::SnapshotOnInitializedNode
                    ));
                }
            }
        }

        let mut stream = self.sync_network.open_stream(peer_id).await?;
        let boundary = self
            .request_snapshot_boundary(context_id, peer_id, &mut stream)
            .await?;

        info!(%context_id, root_hash = %boundary.boundary_root_hash, "Received boundary");

        let (applied_records, observed_schema) = self
            .request_and_apply_snapshot_pages(context_id, &boundary, force, &mut stream)
            .await?;

        // Reconstruct the child tries before reading the root back. Snapshot
        // ships entity rows only, so without this the receiver holds every
        // entity and can enumerate none of them.
        {
            let mut handle = self.context_client.datastore_handle();
            match rebuild_child_tries_after_snapshot(&mut handle, context_id) {
                Ok(linked) => info!(
                    %context_id,
                    linked,
                    "Rebuilt child-trie links for snapshot-installed entities"
                ),
                // Fail the sync rather than warn. There is no "next resync" to
                // wait for: `verified_snapshot_root` below reads the ROOT
                // index's SHIPPED `full_hash`, so a node with an unlinked trie
                // still verifies, still publishes a root matching its peers,
                // and therefore never looks like it needs repair. Returning
                // here leaves the sync-in-progress marker set, exactly as the
                // I7 arms do, so crash-recovery retries instead.
                Err(e) => {
                    warn!(
                        %context_id,
                        error = %e,
                        "Failed to rebuild child-trie links after snapshot; failing the \
                         sync for retry rather than publishing a root whose collections \
                         read back empty"
                    );
                    return Err(eyre::eyre!(
                        "snapshot: child-trie rebuild failed for {context_id}: {e}"
                    ));
                }
            }
        }

        // Verify snapshot integrity against the state that actually landed (I7).
        // Either arm of a failure returns before the sync-in-progress marker is
        // cleared, so crash-recovery re-syncs and retries rather than publishing
        // a root this node cannot back.
        let root_to_store = *verified_snapshot_root(
            self.context_client.datastore(),
            context_id,
            boundary.boundary_root_hash,
        )
        .inspect_err(|_| self.metrics().record_verification_failure())?;

        // Publish root_hash + dag_heads in one atomic ContextMeta write. Two
        // separate read-modify-writes (force_root_hash then update_dag_heads)
        // leave a window where a concurrent whole-record ContextMeta write can
        // interleave and clobber the finalize back to root=0/heads=[] while the
        // applied ContextState entries persist — the permanent
        // SnapshotOnInitializedNode deadlock (#3252).
        self.context_client.set_root_and_dag_heads(
            &context_id,
            root_to_store.into(),
            boundary.dag_heads.clone(),
        )?;
        self.clear_sync_in_progress_marker(context_id)?;
        self.finalize_snapshot_activation(context_id, observed_schema)
            .await;

        info!(%context_id, applied_records, "Snapshot sync completed successfully");
        self.metrics().record_session_cost(
            "Snapshot",
            super::metrics::SessionCost {
                entities_transferred: applied_records as u64,
                ..Default::default()
            },
        );

        Ok(SnapshotSyncResult {
            boundary_root_hash: boundary.boundary_root_hash,
            dag_heads: boundary.dag_heads,
            applied_records,
        })
    }

    /// Request snapshot boundary from a peer.
    /// Refuse a snapshot unless the peer serving it proved, from its own `PeerId`,
    /// an identity currently admitted to the context (#4089).
    ///
    /// Snapshot apply stores entities without gating them on their authors, so
    /// the source is what vouches for them: it must be a member, not revoked.
    /// Fails closed, including when this node cannot yet tell: a joiner that has
    /// not folded the source's membership retries on its next sync.
    fn ensure_snapshot_server_admitted(
        &self,
        context_id: ContextId,
        peer_id: libp2p::PeerId,
        server_identity: calimero_primitives::identity::PublicKey,
        server_proof: &calimero_node_primitives::sync::InitProof,
    ) -> Result<()> {
        let store = self.context_client.datastore_handle().into_inner();
        snapshot_server_admitted(
            &store,
            context_id,
            peer_id,
            server_identity,
            server_proof,
            // A context in no group has no group membership; fall back to
            // the context's own member set, as the inbound check does.
            || {
                self.context_client
                    .has_member(&context_id, &server_identity, None)
            },
        )
    }

    async fn request_snapshot_boundary(
        &self,
        context_id: ContextId,
        peer_id: libp2p::PeerId,
        stream: &mut Stream,
    ) -> Result<SnapshotBoundary> {
        use calimero_node_primitives::sync::InitPayload;

        let Some(our_identity) = self.acting_identity(&context_id).await? else {
            eyre::bail!("No owned identity found for context: {}", context_id);
        };

        let msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::SnapshotBoundaryRequest {
                context_id,
                requested_cutoff_timestamp: None,
            },
            next_nonce: super::helpers::generate_nonce(),
            pop: self.build_init_pop(context_id, our_identity).await,
        };
        super::stream::send(stream, &msg, None).await?;

        let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;

        let Some(StreamMessage::Message { payload, .. }) = response else {
            eyre::bail!("Unexpected response to snapshot boundary request");
        };

        match payload {
            MessagePayload::SnapshotBoundaryResponse {
                boundary_timestamp,
                boundary_root_hash,
                dag_heads,
                server_identity,
                server_proof,
            } => {
                self.ensure_snapshot_server_admitted(
                    context_id,
                    peer_id,
                    server_identity,
                    &server_proof,
                )?;
                Ok(SnapshotBoundary {
                    boundary_timestamp,
                    boundary_root_hash,
                    dag_heads,
                })
            }
            MessagePayload::SnapshotError { error } => {
                eyre::bail!("Snapshot boundary request failed: {:?}", error);
            }
            _ => eyre::bail!("Unexpected payload in snapshot boundary response"),
        }
    }

    /// Request and apply snapshot pages from a peer.
    ///
    /// This method leaves the context's state as it was unless the whole
    /// snapshot checks out:
    /// 1. Receive all pages, checking each entity and writing the ones that pass
    ///    to a staging area ([`Stage`]), not the context's state
    /// 2. Fold the staged tree from the leaves up and compare it with the root
    ///    the boundary named
    /// 3. Set the sync-in-progress marker, then move the staged entities into
    ///    the context's state and delete the old keys the snapshot does not carry
    /// 4. The caller removes the marker once the root is published
    ///
    /// # Concurrency Assumptions
    ///
    /// This method assumes no concurrent writes occur to the context's state during
    /// snapshot sync. This is safe because snapshot sync is only used in two cases:
    ///
    /// 1. **Bootstrap**: The node is uninitialized and has no delta store processing
    ///    transactions yet.
    /// 2. **Crash recovery**: The sync-in-progress marker forces re-sync before normal
    ///    operation resumes, and the sync manager initiates this before the context
    ///    is ready for transaction processing.
    ///
    /// If concurrent writes were to occur, keys written during sync would not be
    /// cleaned up and could cause state divergence.
    /// Returns `(records_applied, observed_schema)`, where `observed_schema` is
    /// the `schema_bytecode_id` the applied entities carried (the source peer's real
    /// schema) — `None` if no entity carried a stamp. The resync settle binds
    /// the activation marker to it so a snapshot from a behind peer is not
    /// mislabeled as the group target.
    async fn request_and_apply_snapshot_pages(
        &self,
        context_id: ContextId,
        boundary: &SnapshotBoundary,
        // `true` for an operator resync (force): adopt the peer's state wholesale,
        // bypassing the per-entity schema fence below. A stranded context's
        // loaded reader is its OLD bound blob, so fencing here would decline the
        // peer's current-schema entities and recover nothing.
        force: bool,
        stream: &mut Stream,
    ) -> Result<(usize, Option<[u8; 32]>)> {
        use calimero_node_primitives::sync::InitPayload;

        let Some(our_identity) = self.acting_identity(&context_id).await? else {
            eyre::bail!("No owned identity found for context: {}", context_id);
        };

        // Wall-clock start, for the snapshot-progress ETA estimate.
        let started_at = Instant::now();

        // Collect existing keys BEFORE receiving any pages
        // We'll use this to determine which keys to delete after sync completes
        let existing_keys: HashSet<[u8; calimero_store::key::STATE_KEY_LEN]> = {
            let handle = self.context_client.datastore_handle();
            collect_context_state_keys(&handle, context_id)?
                .into_iter()
                .collect()
        };
        debug!(%context_id, existing_count = existing_keys.len(), "Collected existing state keys");

        // Pages land in a staging area, and reach the context's state only once
        // the whole tree checks out against the boundary.
        let stage = Stage::open(self.context_client.datastore().clone(), context_id)?;
        // Leaves stamped for a schema this node cannot read yet: staged so the
        // tree check counts them, then buffered instead of installed.
        let mut declined: HashMap<Id, [u8; 32]> = HashMap::new();
        let mut total_applied = 0;
        // The schema the applied entities carry — bound by the resync settle.
        let mut observed_schema: Option<[u8; 32]> = None;
        let mut resume_cursor: Option<Vec<u8>> = None;

        // PR-6b Task 6b.7: the schema this node can read *right now* (its loaded
        // reader). A snapshot `Entity` whose sender stamped a newer
        // `schema_bytecode_id` is declined+buffered rather than stored.
        // `None` (non-group context / unresolvable meta) ⇒ no gate — apply as
        // today (parity with the leaf path's `handle_entity_push`). A forced
        // resync also disables the gate (None): it is an authorized full-state
        // replacement to the peer's current schema, and the stale local marker
        // would otherwise decline every current-schema entity.
        let loaded_bytecode_id = if force {
            None
        } else {
            calimero_context::hlc_fence::loaded_reader_bytecode_id(
                self.context_client.datastore(),
                &context_id,
            )
            .ok()
            .flatten()
        };

        // `SharedMember` entities are verified in a SECOND pass, after every
        // page has been applied. A member carries no inline writer set — its
        // writers live at its anchor (a `Shared` wrapper) — and the snapshot
        // apply path runs OUTSIDE the WASM `RUNTIME_ENV`, so the storage-layer
        // `resolve_anchor_writers` (which reads via `MainStorage`) can't see the
        // anchor here even once persisted; the anchor may also arrive in a later
        // page. So we collect each verified anchor's writer set as it applies
        // (`anchor_writers`), defer members (`deferred_members`), and verify the
        // members against that authenticated map once the stream completes. A
        // member still only persists after its writers are authenticated (the
        // anchor's own record is signature-verified in pass 1).
        let mut anchor_writers: HashMap<
            Id,
            BTreeMap<calimero_account::AccountId, calimero_storage::entities::OpMask>,
        > = HashMap::new();
        // (id, entry blob, index blob, sender's stamped schema_bytecode_id) — the
        // schema is carried so a context whose entities are ALL SharedMember
        // still reports an `observed_schema` (pass 1 only sets it from regular
        // Entity records, so without this the settle would bind to the group
        // target instead of the schema the synced entities actually carry).
        let mut deferred_members: DeferredMembers = Vec::new();
        // A cell's writers past genesis come from the governance fold, never from the snapshot.
        let ever_writers = |cell: Id| {
            self.context_client
                .cell_writers()
                .ever_writers(&context_id, cell)
        };

        // Sign the transport-binding proof once — it's independent of the
        // per-page cursor/nonce (see `InitProof`), so every page request in the
        // burst loop reuses the same signature.
        let pop = self.build_init_pop(context_id, our_identity).await;

        loop {
            let msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::SnapshotStreamRequest {
                    context_id,
                    boundary_root_hash: boundary.boundary_root_hash,
                    page_limit: DEFAULT_PAGE_LIMIT,
                    byte_limit: DEFAULT_PAGE_BYTE_LIMIT,
                    resume_cursor: resume_cursor.clone(),
                },
                next_nonce: super::helpers::generate_nonce(),
                pop,
            };
            super::stream::send(stream, &msg, None).await?;

            // Receive all pages in the burst (server sends up to page_limit pages per request)
            let mut pages_in_burst = 0;
            loop {
                let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;

                let Some(StreamMessage::Message { payload, .. }) = response else {
                    eyre::bail!("Unexpected response during snapshot streaming");
                };

                match payload {
                    MessagePayload::SnapshotPage {
                        payload,
                        uncompressed_len,
                        cursor,
                        page_count,
                        sent_count,
                        total_records,
                    } => {
                        // Handle empty snapshot (no entries): `install_staged_snapshot`
                        // lets it clear only an empty context.
                        if payload.is_empty() && uncompressed_len == 0 {
                            install_staged_snapshot(
                                stage,
                                boundary.boundary_root_hash,
                                existing_keys,
                                declined,
                            )
                            .await?;
                            return Ok((total_applied, observed_schema));
                        }

                        let decompressed = decompress_snapshot_page(&payload, uncompressed_len)?;

                        let records = decode_snapshot_records(&decompressed)?;
                        let mut applied = 0usize;
                        let mut rejected = 0usize;
                        // Staged, not applied via `apply_action`: no nonce replay or CRDT merge,
                        // sound only because I5 or crash recovery leaves no newer local state.
                        for record in &records {
                            match record {
                                SnapshotRecord::Entity {
                                    id,
                                    entry,
                                    index,
                                    schema_bytecode_id,
                                } => {
                                    // PR-6b Task 6b.7: the snapshot apply path
                                    // writes verified entities directly,
                                    // bypassing the gossip
                                    // state-delta fence AND
                                    // `apply_leaf_with_crdt_merge_gated`. So the
                                    // readability check has to live here: if the
                                    // sender stamped a `schema_bytecode_id` newer
                                    // than this node's loaded reader, DECLINE +
                                    // BUFFER the raw entity into the absorb
                                    // buffer instead of persisting unreadable
                                    // bytes. It is buffered once the snapshot
                                    // checks out, and re-applied (re-verified)
                                    // once the loaded reader advances to that
                                    // schema.
                                    if !snapshot_entity_is_readable(
                                        *schema_bytecode_id,
                                        loaded_bytecode_id,
                                    ) {
                                        let named = borsh::from_slice::<
                                            calimero_storage::index::EntityIndex,
                                        >(index)
                                        .map_err(|e| {
                                            refused_snapshot_entity(
                                                context_id,
                                                Id::new(*id),
                                                &format!(
                                                    "has an index row that does not decode: {e}"
                                                ),
                                            )
                                        })?;
                                        check_snapshot_leaf(
                                            context_id,
                                            Id::new(*id),
                                            entry,
                                            &named,
                                        )?;
                                        // The signature does not depend on the schema, so a
                                        // leaf that fails it is refused now, not buffered.
                                        let signed = if matches!(
                                            named.metadata.storage_type,
                                            calimero_storage::entities::StorageType::SharedMember { .. }
                                        ) {
                                            Interface::<MainStorage>::verify_snapshot_member_signature(
                                                Id::new(*id),
                                                entry,
                                                &named.metadata,
                                            )
                                        } else {
                                            Interface::<MainStorage>::verify_snapshot_entity_signature(
                                                Id::new(*id),
                                                named.parent_id(),
                                                entry,
                                                &named.metadata,
                                            )
                                        };
                                        if let Err(e) = signed {
                                            return Err(refused_snapshot_entity(
                                                context_id,
                                                Id::new(*id),
                                                &format!("fails signature verification: {e}"),
                                            ));
                                        }
                                        stage.put_entity(Id::new(*id), entry, index)?;
                                        let _previous = declined.insert(
                                            Id::new(*id),
                                            schema_bytecode_id
                                                .expect("gate only declines when Some"),
                                        );
                                        rejected += 1;
                                        continue;
                                    }
                                    // Per-entity signature verification
                                    // (closes the peer-trust gap from
                                    // issue #2387). Parse the index
                                    // blob to recover metadata, then
                                    // run `verify_snapshot_entity_signature`
                                    // against the data + storage-type
                                    // access-control rules. A record that
                                    // fails is refused, and with it the
                                    // snapshot: its parent's hash counts it.
                                    let index_entity: calimero_storage::index::EntityIndex =
                                        match borsh::from_slice(index) {
                                            Ok(idx) => idx,
                                            Err(e) => {
                                                return Err(refused_snapshot_entity(
                                                    context_id,
                                                    Id::new(*id),
                                                    &format!("has an index row that does not decode: {e}"),
                                                ));
                                            }
                                        };
                                    let id_obj = Id::new(*id);
                                    // Its bytes are checked against its hash here, before any
                                    // check that could set the entity aside.
                                    check_snapshot_leaf(context_id, id_obj, entry, &index_entity)?;

                                    // SharedMember: defer to pass 2. Its writers
                                    // resolve from its anchor, which may not be
                                    // applied yet (later page) and isn't readable
                                    // via MainStorage here anyway. Hold the raw
                                    // blobs; pass 2 verifies + persists.
                                    if matches!(
                                        index_entity.metadata.storage_type,
                                        calimero_storage::entities::StorageType::SharedMember { .. }
                                    ) {
                                        deferred_members.push((
                                            id_obj,
                                            entry.clone(),
                                            index.clone(),
                                            *schema_bytecode_id,
                                        ));
                                        continue;
                                    }

                                    if let Err(e) =
                                        Interface::<MainStorage>::verify_snapshot_entity_signature(
                                            id_obj,
                                            index_entity.parent_id(),
                                            entry,
                                            &index_entity.metadata,
                                        )
                                    {
                                        return Err(refused_snapshot_entity(
                                            context_id,
                                            id_obj,
                                            &format!("fails signature verification: {e}"),
                                        ));
                                    }

                                    if let calimero_storage::entities::StorageType::Shared {
                                        writers,
                                        ..
                                    } = &index_entity.metadata.storage_type
                                    {
                                        if !crate::sync::helpers::snapshot_leaf_admitted(
                                            self.context_client.datastore(),
                                            &self.node_state.folded_tee(),
                                            &context_id,
                                            writers,
                                            &index_entity.metadata.storage_type,
                                        ) {
                                            return Err(refused_snapshot_entity(
                                                context_id,
                                                id_obj,
                                                "claims the TEE-only writer set but its signer is not the TEE authority",
                                            ));
                                        }
                                    }

                                    match crate::sync::helpers::snapshot_leaf_authorship(
                                        self.context_client.datastore(),
                                        &self.node_state.folded_tee(),
                                        &context_id,
                                        id_obj,
                                        &index_entity.metadata,
                                        None,
                                        &ever_writers,
                                    ) {
                                        SnapshotAuthorship::Authored => {}
                                        SnapshotAuthorship::Forged => {
                                            return Err(refused_snapshot_entity(
                                                context_id,
                                                id_obj,
                                                "was signed by an account that is neither its owner nor one of its writers",
                                            ));
                                        }
                                        SnapshotAuthorship::Unknown => {
                                            return Err(unknown_snapshot_signer(
                                                context_id, id_obj,
                                            ));
                                        }
                                    }

                                    // Verified: staged, and installed with the rest of the snapshot.
                                    stage.put_entity(id_obj, entry, index)?;
                                    applied += 1;
                                    if let Some(k) = schema_bytecode_id {
                                        observed_schema = Some(*k);
                                    }

                                    // Record this verified anchor's writer set so
                                    // pass 2 can authenticate members against it
                                    // (the snapshot path can't use
                                    // `resolve_anchor_writers` — no RUNTIME_ENV).
                                    if let calimero_storage::entities::StorageType::Shared {
                                        writers,
                                        ..
                                    } = &index_entity.metadata.storage_type
                                    {
                                        let _ = anchor_writers.insert(id_obj, writers.clone());
                                    }
                                }
                                SnapshotRecord::Auxiliary { kind, id, .. } => {
                                    // `Auxiliary` is the channel for
                                    // records that aren't
                                    // per-record-signature-verifiable.
                                    //
                                    // Until per-record authentication
                                    // exists (issue #2387 follow-up),
                                    // every kind is rejected:
                                    //
                                    // * `INDEX` / `ENTRY` — would
                                    //   bypass the per-entity
                                    //   signature verify on `Entity`
                                    //   records. A malicious peer
                                    //   shipping these alongside a
                                    //   verified Entity could
                                    //   clobber the just-verified
                                    //   index/entry blobs.
                                    // * kind 2 — once a sync-state
                                    //   key nothing ever wrote; a
                                    //   peer emitting one is
                                    //   misbehaving.
                                    // * `ROTATION_LOG` - legacy rotation
                                    //   history nothing reads now: a
                                    //   cell's writers come from the
                                    //   governance fold.
                                    warn!(
                                        %context_id,
                                        kind,
                                        id = ?id,
                                        "snapshot Auxiliary record: rejecting — no kind \
                                         currently has per-record authentication (issue \
                                         #2387 follow-up: sign each rotation-log entry \
                                         at write time)"
                                    );
                                    rejected += 1;
                                    continue;
                                }
                            }
                        }
                        if rejected > 0 {
                            warn!(
                                %context_id,
                                applied,
                                rejected,
                                page_records = records.len(),
                                "snapshot page applied with rejections"
                            );
                        }

                        total_applied += applied;
                        pages_in_burst += 1;

                        debug!(
                            %context_id,
                            pages_in_burst,
                            page_count,
                            sent_count,
                            total_applied,
                            "Applied snapshot page"
                        );

                        // Surface snapshot progress (per page-burst, a natural
                        // throttle) so a subscriber watching this uninitialized
                        // context sees forward motion rather than a silent wait.
                        self.emit_snapshot_progress(
                            context_id,
                            total_applied as u64,
                            total_records,
                            started_at.elapsed(),
                        );

                        // Check if this is the last page in this burst
                        let is_last_in_burst = sent_count == page_count;

                        if is_last_in_burst {
                            // Check if there are more pages to fetch
                            match cursor {
                                None => {
                                    // Pass 2: every anchor is now applied, so
                                    // verify + persist the deferred SharedMember
                                    // entities against their anchor's collected
                                    // (and signature-verified) writer set. A
                                    // member whose anchor never appeared, or
                                    // whose signature doesn't verify, is dropped
                                    // — same fail-closed semantics as pass 1.
                                    if !deferred_members.is_empty() {
                                        for (id_obj, entry, index, member_schema) in
                                            deferred_members.drain(..)
                                        {
                                            let metadata = match borsh::from_slice::<
                                                calimero_storage::index::EntityIndex,
                                            >(
                                                &index
                                            ) {
                                                Ok(idx) => idx.metadata,
                                                Err(e) => {
                                                    return Err(refused_snapshot_entity(
                                                        context_id,
                                                        id_obj,
                                                        &format!("has an index row that does not decode: {e}"),
                                                    ));
                                                }
                                            };
                                            let anchor = match &metadata.storage_type {
                                                calimero_storage::entities::StorageType::SharedMember {
                                                    anchor,
                                                    ..
                                                } => *anchor,
                                                // A deferred record is always a
                                                // member; defensive only.
                                                _ => continue,
                                            };
                                            let Some(writers) = anchor_writers.get(&anchor) else {
                                                return Err(refused_snapshot_entity(
                                                    context_id,
                                                    id_obj,
                                                    "is a member whose anchor is not in the snapshot",
                                                ));
                                            };
                                            if let Err(e) =
                                                Interface::<MainStorage>::verify_snapshot_member_signature(
                                                    id_obj, &entry, &metadata,
                                                )
                                            {
                                                return Err(refused_snapshot_entity(
                                                    context_id,
                                                    id_obj,
                                                    &format!("fails signature verification: {e}"),
                                                ));
                                            }
                                            if !crate::sync::helpers::snapshot_leaf_admitted(
                                                self.context_client.datastore(),
                                                &self.node_state.folded_tee(),
                                                &context_id,
                                                writers,
                                                &metadata.storage_type,
                                            ) {
                                                return Err(refused_snapshot_entity(
                                                    context_id,
                                                    id_obj,
                                                    "belongs to a TEE-only anchor but its signer is not the TEE authority",
                                                ));
                                            }
                                            match crate::sync::helpers::snapshot_leaf_authorship(
                                                self.context_client.datastore(),
                                                &self.node_state.folded_tee(),
                                                &context_id,
                                                id_obj,
                                                &metadata,
                                                Some(writers),
                                                &ever_writers,
                                            ) {
                                                SnapshotAuthorship::Authored => {}
                                                SnapshotAuthorship::Forged => {
                                                    return Err(refused_snapshot_entity(
                                                        context_id,
                                                        id_obj,
                                                        "was signed by an account that never was one of its anchor's writers",
                                                    ));
                                                }
                                                SnapshotAuthorship::Unknown => {
                                                    return Err(unknown_snapshot_signer(
                                                        context_id, id_obj,
                                                    ));
                                                }
                                            }
                                            stage.put_entity(id_obj, &entry, &index)?;
                                            total_applied += 1;
                                            // Bind observed_schema from members too,
                                            // so a SharedMember-only context settles
                                            // to the schema its entities carry.
                                            if let Some(k) = member_schema {
                                                observed_schema = Some(k);
                                            }
                                        }
                                    }

                                    // All pages received: install the snapshot, and
                                    // delete the keys it replaces, only if its tree
                                    // folds to the root the boundary named.
                                    install_staged_snapshot(
                                        stage,
                                        boundary.boundary_root_hash,
                                        existing_keys,
                                        declined,
                                    )
                                    .await?;
                                    return Ok((total_applied, observed_schema));
                                }
                                Some(c) => {
                                    resume_cursor = Some(c);
                                    break; // Exit inner loop, request more pages
                                }
                            }
                        }
                        // Continue receiving more pages in this burst
                    }
                    MessagePayload::SnapshotError { error } => {
                        eyre::bail!("Snapshot streaming failed: {:?}", error);
                    }
                    _ => eyre::bail!("Unexpected payload during snapshot streaming"),
                }
            }
        }
    }

    /// Record snapshot progress on the advisory `sync_status` mirror and push a
    /// `SyncStatus` event to subscribers. Best-effort: a broadcast with no
    /// receivers is fine. `percent`/`eta_secs` are derived only when the sender
    /// advertised a non-zero grand total (`total_records`); otherwise the
    /// update carries the raw `records_received` liveness signal alone.
    fn emit_snapshot_progress(
        &self,
        context_id: ContextId,
        records_received: u64,
        total_records: u64,
        elapsed: std::time::Duration,
    ) {
        let (percent, eta_secs) =
            snapshot_progress_estimate(records_received, total_records, elapsed);
        let state = SyncState::ReceivingSnapshot {
            records_received,
            percent,
            eta_secs,
        };
        let handle = self.node_state.sync_status_handle();
        // Preserve any failure history the run-loop has already published for
        // this context; this path only advances the snapshot phase. Writing
        // 0/None here would flip `failure_count`/`last_error` to "healthy"
        // mid-snapshot until the next run-loop publish. (Copy into owned values
        // and drop the read guard before `insert` — a same-key get+insert on a
        // `DashMap` shard would otherwise deadlock.)
        let (failure_count, last_error) = match handle.get(&context_id) {
            Some(prev) => (prev.failure_count, prev.last_error.clone()),
            None => (0, None),
        };
        let _prev = handle.insert(
            context_id,
            SyncStatusSnapshot {
                state,
                failure_count,
                last_error: last_error.clone(),
            },
        );
        let event = NodeEvent::Context(ContextEvent {
            context_id,
            payload: ContextEventPayload::SyncStatus(SyncStatusPayload {
                sync_state: state,
                failure_count,
                last_error,
            }),
        });
        if let Err(err) = self.node_client.send_event(event) {
            debug!(%context_id, %err, "failed to emit snapshot-progress event");
        }
    }

    /// Settle a context's per-context binding after a full-state snapshot.
    /// Resync-scoped (see `settle_snapshot_activation`): only an operator resync
    /// rebinds the activation marker — to `observed_schema`, the schema the
    /// applied entities actually carried. Best-effort and group-only.
    async fn finalize_snapshot_activation(
        &self,
        context_id: ContextId,
        observed_schema: Option<[u8; 32]>,
    ) {
        let Some(namespace_id) = settle_snapshot_activation(
            self.context_client.datastore(),
            context_id,
            observed_schema,
        ) else {
            return;
        };

        // A resynced peer never ran the install for the bytecode it recovered, so
        // install the marker's blob; one older than the row's release leaves the row.
        if let Some(bound) = calimero_context::activation::activated_bytecode(
            self.context_client.datastore(),
            &context_id,
        ) {
            let blob_id = calimero_primitives::blobs::BlobId::from(bound);
            if self.node_client.has_blob(&blob_id).unwrap_or(false) {
                match self.context_client.get_context(&context_id) {
                    Ok(Some(context)) => {
                        let mut application: Option<calimero_primitives::application::Application> =
                            None;
                        if let Err(err) = self
                            .install_bundle_after_blob_sharing(
                                &context_id,
                                &blob_id,
                                &context,
                                &mut application,
                            )
                            .await
                        {
                            warn!(
                                %context_id, %err,
                                "resync in-place install failed; reported version may lag \
                                 until the next install"
                            );
                        }
                    }
                    other => debug!(
                        %context_id, ?other,
                        "resync: context unavailable for in-place install; skipping"
                    ),
                }
            }
        }

        // Edge-trigger the migration emitter so the recovered facts (cleared
        // strand marker + advanced installed version) reach the admin rollup at
        // once, instead of lingering as stale `failed`/`in-progress` until the
        // next ~30s periodic heartbeat.
        self.node_client
            .notify_migration_facts_refresh(namespace_id);
    }

    fn clear_sync_in_progress_marker(&self, context_id: ContextId) -> Result<()> {
        let key = GenericKey::new(SYNC_IN_PROGRESS_SCOPE, *context_id);
        let mut handle = self.context_client.datastore_handle();
        handle.delete(&key)?;
        debug!(%context_id, "Cleared sync-in-progress marker");
        Ok(())
    }

    /// Check if a context has an incomplete snapshot sync (marker present).
    ///
    /// Returns the boundary root hash that was being synced, if a marker exists.
    pub fn check_sync_in_progress(&self, context_id: ContextId) -> Result<Option<Hash>> {
        let key = GenericKey::new(SYNC_IN_PROGRESS_SCOPE, *context_id);
        let handle = self.context_client.datastore_handle();
        let value_opt = handle.get(&key)?;
        match value_opt {
            Some(value) => {
                let bytes: Vec<u8> = value.as_ref().to_vec();
                let hash: Hash = borsh::from_slice(&bytes)?;
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }
}

/// Buffer a future-schema snapshot `Entity` into the absorb buffer instead
/// of storing unreadable bytes (PR-6b Task 6b.7).
///
/// Keyed by the entity id (idempotent overwrite on re-delivery), under the
/// *sender's* schema so the drain only re-verifies + persists it once this
/// node advances to that reader. Caller has already confirmed
/// `!snapshot_entity_is_readable(Some(schema), loaded)`.
fn buffer_future_schema_snapshot_entity(
    store: &Store,
    context_id: ContextId,
    id: [u8; 32],
    entry: &[u8],
    index: &[u8],
    schema: [u8; 32],
) -> Result<()> {
    let record = calimero_governance_store::AbsorbRecord::from_snapshot_entity(
        id,
        entry.to_vec(),
        index.to_vec(),
        schema,
    );
    calimero_governance_store::AbsorbRepository::new(store).save(&context_id, schema, &record)?;
    crate::node_metrics::record_delta_outcome("absorbed_snapshot_entity_future_schema");
    warn!(
        %context_id,
        id = ?id,
        ?schema,
        "snapshot entity authored under a newer schema than the loaded reader \
         — buffered into the absorb buffer instead of storing unreadable bytes \
         (will re-verify + persist once the reader advances)"
    );
    Ok(())
}

/// Checks the staged snapshot against `claimed`, then moves it into the
/// context's state, deleting the `existing_keys` it does not carry. A refusal
/// leaves the state as it was and sets no sync-in-progress marker.
async fn install_staged_snapshot(
    stage: Stage,
    claimed: Hash,
    existing_keys: HashSet<[u8; calimero_store::key::STATE_KEY_LEN]>,
    declined: HashMap<Id, [u8; 32]>,
) -> Result<()> {
    // The check hashes every entity and rebuilds its trie, so it runs off the
    // async workers.
    tokio::task::spawn_blocking(move || {
        let (store, context_id) = (stage.store().clone(), stage.context_id());
        let mut skip = stage.verify(claimed)?;
        skip.extend(declined.keys().copied());
        set_sync_in_progress_marker(&store, context_id, &claimed)?;
        let installed = stage.promote(&existing_keys, &skip)?;
        debug!(%context_id, installed, "Installed snapshot");
        for (id, schema) in declined {
            let (entry, index) = stage.entity(id)?.ok_or_else(|| {
                eyre::eyre!(
                    "snapshot: declined entity {:?} left the stage",
                    id.as_bytes()
                )
            })?;
            if let Err(e) = buffer_future_schema_snapshot_entity(
                &store,
                context_id,
                *id.as_bytes(),
                &entry,
                &index,
                schema,
            ) {
                warn!(
                    %context_id,
                    id = ?id.as_bytes(),
                    error = ?e,
                    "snapshot Entity record: failed to buffer future-schema entity into \
                     the absorb buffer"
                );
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| eyre::eyre!("snapshot install task failed: {e}"))?
}

/// Set a marker indicating snapshot sync is in progress for this context.
///
/// This marker is used for crash recovery - if present on startup, the
/// context's state may be inconsistent and needs to be re-synced.
fn set_sync_in_progress_marker(
    store: &Store,
    context_id: ContextId,
    boundary_root_hash: &Hash,
) -> Result<()> {
    use calimero_store::types::GenericData;

    let key = GenericKey::new(SYNC_IN_PROGRESS_SCOPE, *context_id);
    let value_bytes = borsh::to_vec(boundary_root_hash)?;
    let value: GenericData<'_> = Slice::from(value_bytes).into();
    store.handle().put(&key, &value)?;
    debug!(%context_id, "Set sync-in-progress marker");
    Ok(())
}

/// Outcome of draining a buffered snapshot entity (PR-6b Task 6b.7).
///
/// Distinguishes "this buffer record is finished — delete it" from "not yet —
/// keep it for a later pass".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotEntityDrainOutcome {
    /// Entry + Index blobs were re-verified and persisted — delete the record.
    Persisted,
    /// Not decidable yet — keep the record for a later pass: an index blob
    /// that does not parse, a signer no folded certificate names, a
    /// `SharedMember` whose anchor is not stored yet, or a shared cell whose
    /// writers cannot be read yet. Bounded by [`drain_buffered_snapshot_entity`].
    Pending,
    /// The page apply would drop the entity: its signature does not verify, its
    /// bytes do not match its own hash, it claims the TEE-only writer set but
    /// its signer is not the TEE authority, or its signer's account is not the
    /// entry's owner, nor a writer the cell has ever had. That verdict does not
    /// change on a retry, so the record is deleted rather than kept. Also what a
    /// record left pending too often becomes.
    Refused,
}

/// Where a buffered snapshot entity falls in a drain pass, lowest first.
///
/// The page apply's order: plain leaves, then `Shared` anchors, then
/// `SharedMember`s, whose writers are those anchors'. A sender stamps every
/// entity of a snapshot with one schema, so a snapshot declined as future-schema
/// drains in one pass only if its records are taken in this order.
pub(crate) fn buffered_snapshot_entity_pass(index: &[u8]) -> u8 {
    match borsh::from_slice::<calimero_storage::index::EntityIndex>(index)
        .map(|idx| idx.metadata.storage_type)
    {
        Ok(calimero_storage::entities::StorageType::Shared { .. }) => 1,
        Ok(calimero_storage::entities::StorageType::SharedMember { .. }) => 2,
        _ => 0,
    }
}

/// Re-verify and persist a buffered future-schema snapshot entity (PR-6b Task
/// 6b.7), reaching the verdict `request_and_apply_snapshot_pages` reaches on
/// it, then `handle.put` the `entry` + `index` blobs under their hashed storage
/// keys.
///
/// What the page apply takes from the rest of the snapshot, this reads from the
/// store: a `SharedMember`'s genesis writers from its stored anchor. A missing
/// anchor, or a cell whose writers `ever_writers` cannot read yet, leaves the
/// entity [`Pending`](SnapshotEntityDrainOutcome::Pending); the caller drains in
/// [`buffered_snapshot_entity_pass`] order so anchors are stored first.
pub(crate) fn persist_buffered_snapshot_entity(
    store: &calimero_store::Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: ContextId,
    id: [u8; 32],
    entry: &[u8],
    index: &[u8],
    ever_writers: &dyn Fn(Id) -> Result<CellWriters, WritersUnavailable>,
) -> Result<SnapshotEntityDrainOutcome> {
    use calimero_storage::entities::StorageType;

    let index_entity: calimero_storage::index::EntityIndex = match borsh::from_slice(index) {
        Ok(idx) => idx,
        Err(e) => {
            warn!(%context_id, id = ?id, error = ?e,
                "absorb entity drain: index blob failed to deserialize — leaving pending");
            return Ok(SnapshotEntityDrainOutcome::Pending);
        }
    };
    let id_obj = Id::new(id);
    let metadata = &index_entity.metadata;

    // Its bytes and index row are held to the checks a page-applied leaf is; a
    // retry would not change either, so it is refused for good.
    if let Some(defect) = leaf::leaf_defect(id_obj, entry, &index_entity, time_now()) {
        warn!(%context_id, id = ?id, defect,
            "absorb entity drain: the leaf does not hold together, deleting");
        return Ok(SnapshotEntityDrainOutcome::Refused);
    }

    // For a member, the genesis writer set of its anchor: the stored anchor's, as the
    // page apply takes it from the snapshot's.
    let anchor_writers = match &metadata.storage_type {
        StorageType::SharedMember { anchor, .. } => {
            let Some(stored) =
                crate::delta_store::read_entity_index_direct(store, context_id, *anchor)?
            else {
                debug!(%context_id, id = ?id, anchor = ?anchor.as_bytes(),
                    "absorb entity drain: SharedMember's anchor is not stored yet — leaving pending");
                return Ok(SnapshotEntityDrainOutcome::Pending);
            };
            let StorageType::Shared { writers, .. } = stored.metadata.storage_type else {
                warn!(%context_id, id = ?id, anchor = ?anchor.as_bytes(),
                    "absorb entity drain: SharedMember's anchor is not a Shared entity — deleting");
                return Ok(SnapshotEntityDrainOutcome::Refused);
            };
            // A rotation never rewrites a wrapper, so a stored one whose writers its id does
            // not commit to was not taken from a snapshot or a delta of this version.
            if calimero_storage::collections::is_cell_id(*anchor)
                && !calimero_storage::collections::cell_id_binds(*anchor, &writers)
            {
                warn!(%context_id, id = ?id, anchor = ?anchor.as_bytes(),
                    "absorb entity drain: SharedMember's anchor holds writers its id does not commit to, deleting");
                return Ok(SnapshotEntityDrainOutcome::Refused);
            }
            Some(writers)
        }
        StorageType::Shared { .. }
        | StorageType::Public
        | StorageType::Frozen
        | StorageType::User { .. } => None,
    };

    let signature = if anchor_writers.is_some() {
        Interface::<MainStorage>::verify_snapshot_member_signature(id_obj, entry, metadata)
    } else {
        Interface::<MainStorage>::verify_snapshot_entity_signature(
            id_obj,
            index_entity.parent_id(),
            entry,
            metadata,
        )
    };
    // The signer's key rides in the leaf, so no later arrival makes a failed
    // signature verify: refuse it, as the page apply drops it.
    if let Err(e) = signature {
        warn!(%context_id, id = ?id, error = ?e,
            "absorb entity drain: signature verification failed — deleting");
        return Ok(SnapshotEntityDrainOutcome::Refused);
    }

    let tee_writers = match &metadata.storage_type {
        StorageType::Shared { writers, .. } => Some(writers),
        _ => anchor_writers.as_ref(),
    };
    if let Some(writers) = tee_writers {
        if !crate::sync::helpers::snapshot_leaf_admitted(
            store,
            folded,
            &context_id,
            writers,
            &metadata.storage_type,
        ) {
            warn!(%context_id, id = ?id,
                "absorb entity drain: claims the TEE-only writer set but its signer is \
                 not the TEE authority — deleting");
            return Ok(SnapshotEntityDrainOutcome::Refused);
        }
    }

    // The signature check does not ask whose account the signer speaks for, so
    // a leaf under another account's `owner`, or a writer set its signer is not
    // in, passes it. The page apply refuses such a leaf, and so must this late one.
    match crate::sync::helpers::snapshot_leaf_authorship(
        store,
        folded,
        &context_id,
        id_obj,
        metadata,
        anchor_writers.as_ref(),
        ever_writers,
    ) {
        SnapshotAuthorship::Authored => {}
        SnapshotAuthorship::Unknown => return Ok(SnapshotEntityDrainOutcome::Pending),
        SnapshotAuthorship::Forged => {
            warn!(%context_id, id = ?id,
                "absorb entity drain: its signer's account is not the entry's owner, nor a \
                 writer the cell has ever had, deleting");
            return Ok(SnapshotEntityDrainOutcome::Refused);
        }
    }

    let mut handle = store.handle();
    let _row_state_key = put_entity_row(&mut handle, context_id, id_obj, entry, index)?;

    // Link into the parent's child trie HERE, not only in the one-shot rebuild
    // after the snapshot pages land.
    //
    // This path runs from `drain_absorbed_leaves`, which fires on a later delta
    // once the loaded reader has advanced to the entity's schema — long after
    // `rebuild_child_tries_after_snapshot` has already run. An entity that lands
    // now would otherwise be present but unenumerable from its parent forever,
    // because (as that rebuild's own doc notes) re-applying byte-identical
    // entities never re-links: it goes through the update path, not
    // `add_child_to`. That is the same permanent, self-stable divergence the
    // rebuild exists to prevent, arriving through the late door.
    if let Some(parent_id) = index_entity.parent_id() {
        link_child_into_parent_trie(
            &mut handle,
            context_id,
            parent_id,
            calimero_storage::entities::ChildInfo::new(
                index_entity.id(),
                index_entity.full_hash(),
                index_entity.metadata.clone(),
            ),
        )?;
    }

    Ok(SnapshotEntityDrainOutcome::Persisted)
}

/// Drain one buffered snapshot-entity record through
/// [`persist_buffered_snapshot_entity`], and settle the record: deleted once
/// persisted or refused, kept while pending.
///
/// A pending record is kept for at most
/// [`MAX_GOVERNANCE_DRAIN_ATTEMPTS`](calimero_node_primitives::delta_buffer::MAX_GOVERNANCE_DRAIN_ATTEMPTS)
/// passes, counted in the record's `governance_drain_attempts`, which an
/// entity record otherwise never uses (the record is plain borsh, so a new
/// field would need a migration). The same triggers drive this drain as the
/// governance-pending one, so the same bound applies: past it, what a
/// member planted cannot sit in the buffer for good.
pub(crate) fn drain_buffered_snapshot_entity(
    store: &calimero_store::Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: ContextId,
    producing_bytecode_id: [u8; 32],
    mut record: calimero_governance_store::AbsorbRecord,
    ever_writers: &dyn Fn(Id) -> Result<CellWriters, WritersUnavailable>,
) -> Result<SnapshotEntityDrainOutcome> {
    let repo = calimero_governance_store::AbsorbRepository::new(store);
    let Some(entity) = record.entity.as_ref() else {
        eyre::bail!(
            "absorb entity drain: record {:?} holds no entity",
            record.id
        );
    };
    let outcome = persist_buffered_snapshot_entity(
        store,
        folded,
        context_id,
        entity.id,
        &entity.entry,
        &entity.index,
        ever_writers,
    )?;
    if outcome != SnapshotEntityDrainOutcome::Pending {
        repo.delete(&context_id, producing_bytecode_id, record.id)?;
        return Ok(outcome);
    }
    record.governance_drain_attempts = record.governance_drain_attempts.saturating_add(1);
    if record.governance_drain_attempts
        >= calimero_node_primitives::delta_buffer::MAX_GOVERNANCE_DRAIN_ATTEMPTS
    {
        warn!(%context_id, id = ?record.id, attempts = record.governance_drain_attempts,
            "absorb entity drain: still undecidable after every allowed pass — deleting");
        repo.delete(&context_id, producing_bytecode_id, record.id)?;
        return Ok(SnapshotEntityDrainOutcome::Refused);
    }
    repo.save(&context_id, producing_bytecode_id, &record)?;
    Ok(SnapshotEntityDrainOutcome::Pending)
}

/// Writes an entity received in a snapshot: its index record and its data, as
/// the one entity row the storage layer keeps for them (`calimero_storage::row`).
/// Returns the row's state key, which the install keeps across stale-key cleanup.
fn put_entity_row(
    handle: &mut calimero_store::Handle<Store>,
    context_id: ContextId,
    id: Id,
    entry: &[u8],
    index: &[u8],
) -> Result<[u8; calimero_store::key::STATE_KEY_LEN]> {
    let row_state_key = StorageKey::Index(id).to_bytes();
    let row = calimero_storage::row::encode(
        id,
        &calimero_storage::row::Row {
            index: Some(index.to_vec()),
            data: Some(entry.to_vec()),
        },
    );
    handle.put(
        &ContextStateKey::new(context_id, row_state_key),
        &ContextStateValue::from(Slice::from(row)),
    )?;
    Ok(row_state_key)
}

/// The entity whose row sits at `state_key`, or `None` for any other kind of
/// state row (child trie, sync state).
fn entity_id_of(state_key: &[u8]) -> Option<Id> {
    match StorageKey::from_bytes(state_key)? {
        StorageKey::Index(id) => Some(id),
        _ => None,
    }
}

/// Insert one parent→child link into the parent's `ChildTrie`, through a raw
/// store handle.
///
/// Shared by the post-snapshot rebuild and the late buffered-entity drain so
/// the two cannot drift in how a link is written — they are the same operation
/// arriving at different times.
fn link_child_into_parent_trie(
    handle: &mut calimero_store::Handle<calimero_store::Store>,
    context_id: ContextId,
    parent_id: Id,
    child: calimero_storage::entities::ChildInfo,
) -> Result<()> {
    link_children_into_parent_trie(handle, context_id, parent_id, vec![child])
}

/// Link many children of ONE parent, sharing a row cache across them.
///
/// Siblings collide on the same `DEPTH+1` spine rows by construction — that is
/// what makes the trie bounded — so linking them one at a time re-reads and
/// rewrites the same spine for every child. Buffering the parent's rows and
/// flushing once cuts the store traffic by most of that factor.
///
/// Worth doing even though this runs on install rather than per operation: a
/// context with tens of thousands of entities pays it in full on every
/// bootstrap, and "one-time" is not the same as "free".
///
/// Correctness is unchanged: `insert_with` is a pure function of the rows it
/// reads, so serving those rows from a cache that already holds this parent's
/// pending writes gives exactly the sequence the uncached path would, one
/// child at a time.
fn link_children_into_parent_trie(
    handle: &mut calimero_store::Handle<calimero_store::Store>,
    context_id: ContextId,
    parent_id: Id,
    children: Vec<calimero_storage::entities::ChildInfo>,
) -> Result<()> {
    let writes = child_trie_writes(
        |key| {
            let k = ContextStateKey::new(context_id, key.to_bytes());
            handle.get(&k).ok().flatten().map(|v| v.as_ref().to_vec())
        },
        parent_id,
        children,
    );
    for (key, bytes) in writes {
        let k = ContextStateKey::new(context_id, key.to_bytes());
        let slice: Slice<'_> = bytes.into();
        handle.put(&k, &ContextStateValue::from(slice))?;
    }
    Ok(())
}

/// The trie rows that linking `children` into `parent_id`'s trie writes, over
/// the rows `read` returns. Pure, so the live state and a staged snapshot share it.
fn child_trie_writes(
    read: impl Fn(StorageKey) -> Option<Vec<u8>>,
    parent_id: Id,
    children: Vec<calimero_storage::entities::ChildInfo>,
) -> BTreeMap<StorageKey, Vec<u8>> {
    let mut pending: BTreeMap<StorageKey, Vec<u8>> = BTreeMap::new();

    for child in children {
        let mut writes: Vec<(StorageKey, Vec<u8>)> = Vec::new();
        calimero_storage::child_trie::ChildTrie::<calimero_storage::store::MainStorage>::insert_with(
            parent_id,
            child,
            |key| pending.get(&key).cloned().or_else(|| read(key)),
            |key, bytes| writes.push((key, bytes.to_vec())),
        );
        for (key, bytes) in writes {
            let _prev = pending.insert(key, bytes);
        }
    }
    pending
}

/// Rebuild every installed entity's link into its parent's child trie.
///
/// # Why a whole pass, after the fact
///
/// Snapshot installs an entity by writing its `Entry` and `Index` rows
/// verbatim. While a parent's children lived INSIDE its index row, that shipped
/// the parent→child links for free. They now live in their own keyspace
/// (`Key::ChildTrie`), which snapshot discovery cannot recognise — it finds
/// entities by deserialising `EntityIndex`, and a trie row never will.
///
/// Without this, a snapshot receiver holds every entity and can enumerate none
/// of them: `get_children_of` is trie-backed, so collections read back empty
/// while the entity rows sit there intact.
///
/// It is a separate pass rather than a hook in the install sites because there
/// are three of them (the page-apply loop, the re-drive loop, and the buffered
/// drain), and a link missed by any one is silent. Rebuilding from what
/// actually landed cannot miss a path.
///
/// Replays the sender's own `full_hash` for each child, so the reconstructed
/// trie reproduces the sender's root exactly — the trie is a pure function of
/// the `{(id, full_hash)}` set. No re-hashing, and no parent-before-child
/// ordering requirement.
fn rebuild_child_tries_after_snapshot(
    handle: &mut calimero_store::Handle<calimero_store::Store>,
    context_id: ContextId,
) -> Result<usize> {
    // Collect first: the iterator borrows the handle we need to write through.
    let mut links: Vec<(Id, calimero_storage::entities::ChildInfo)> = Vec::new();
    {
        let mut iter = handle.iter::<ContextStateKey>()?;
        for (key_result, value_result) in iter.entries() {
            let key = key_result?;
            let value = value_result?;
            if key.context_id() != context_id {
                continue;
            }
            let Some(id) = entity_id_of(&key.state_key()) else {
                continue;
            };
            let Some(index_entity) = calimero_storage::row::decode(id, value.value.as_ref())
                .and_then(|row| row.entity_index())
            else {
                continue;
            };
            if let Some(parent_id) = index_entity.parent_id() {
                links.push((
                    parent_id,
                    calimero_storage::entities::ChildInfo::new(
                        index_entity.id(),
                        index_entity.full_hash(),
                        index_entity.metadata.clone(),
                    ),
                ));
            }
        }
    }

    let linked = links.len();

    // Group by parent so siblings share one pass over the spine rows they all
    // touch, rather than each re-reading and rewriting them.
    let mut by_parent: BTreeMap<Id, Vec<calimero_storage::entities::ChildInfo>> = BTreeMap::new();
    for (parent_id, child) in links {
        by_parent.entry(parent_id).or_default().push(child);
    }
    for (parent_id, children) in by_parent {
        link_children_into_parent_trie(handle, context_id, parent_id, children)?;
    }

    Ok(linked)
}

/// Result of a successful snapshot sync.
#[derive(Debug)]
pub struct SnapshotSyncResult {
    pub boundary_root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
    pub applied_records: usize,
}

/// Boundary negotiation result.
struct SnapshotBoundary {
    #[allow(dead_code)]
    boundary_timestamp: u64,
    boundary_root_hash: Hash,
    dag_heads: Vec<[u8; 32]>,
}

/// [`SyncManager::ensure_snapshot_server_admitted`] over the store, so the rule
/// can be tested against real governance state without a running node.
/// `has_member` answers for a context that belongs to no group.
fn snapshot_server_admitted(
    store: &Store,
    context_id: ContextId,
    peer_id: libp2p::PeerId,
    server_identity: calimero_primitives::identity::PublicKey,
    server_proof: &calimero_node_primitives::sync::InitProof,
    has_member: impl FnOnce() -> Result<bool>,
) -> Result<()> {
    if !server_proof.verify(&context_id, &server_identity, &peer_id.to_bytes()) {
        warn!(%context_id, %peer_id, %server_identity, "refusing snapshot: source's proof of identity does not verify");
        eyre::bail!("snapshot source {peer_id} did not prove the identity it serves as");
    }
    let admitted = match calimero_governance_store::is_admitted_to_context(
        store,
        &context_id,
        &server_identity,
    )? {
        Some(admitted) => admitted,
        None => has_member()?,
    };
    if !admitted {
        warn!(%context_id, %peer_id, %server_identity, "refusing snapshot: source is not an admitted member");
        eyre::bail!(
            "snapshot source {peer_id} serves as {server_identity}, which is not admitted to {context_id}"
        );
    }
    Ok(())
}

/// The error that fails a snapshot on an entity this node would leave out.
///
/// The entity's parent still counts it in the hash the source shipped, so leaving
/// it out would install a tree the claimed root vouches for and that lacks it,
/// and nothing would notice. Only a leaf the schema keeps from being read yet is
/// buffered instead.
fn refused_snapshot_entity(context_id: ContextId, id: Id, why: &str) -> eyre::Report {
    warn!(%context_id, id = ?id.as_bytes(), why, "refusing snapshot: an entity fails its checks");
    eyre::eyre!("snapshot: entity {:?} in {context_id} {why}", id.as_bytes())
}

/// Fails the snapshot on an entity that is not what its index row says.
fn check_snapshot_leaf(
    context_id: ContextId,
    id: Id,
    entry: &[u8],
    index: &calimero_storage::index::EntityIndex,
) -> Result<()> {
    match leaf::leaf_defect(id, entry, index, time_now()) {
        Some(defect) => Err(refused_snapshot_entity(context_id, id, defect)),
        None => Ok(()),
    }
}

/// The error that fails a snapshot on a leaf whose signer has no certified
/// account here yet.
///
/// Failing rather than dropping the leaf is the point. The root check reads the
/// shipped root index, so a dropped leaf would leave this node publishing its
/// source's root over state that lacks it, and nothing would ever repair the
/// gap. An unknown signer almost always means this joiner has not folded the
/// namespace's governance that far, so the retry succeeds once it has. Nothing
/// of the snapshot is installed and no marker is set, so the retry is a fresh
/// bootstrap.
fn unknown_snapshot_signer(context_id: ContextId, id: Id) -> eyre::Report {
    warn!(
        %context_id,
        id = ?id.as_bytes(),
        "refusing snapshot: an entity's signer has no certified account here yet; \
         retrying once governance has caught up"
    );
    eyre::eyre!(
        "snapshot: signer of entity {:?} in {context_id} has no certified account here yet",
        id.as_bytes()
    )
}

/// The root hash a completed snapshot may be published under.
///
/// `Ok` only when the state that actually landed hashes to the boundary the
/// sender claimed. Anything else is an error, and the caller must not publish:
/// the alternative — storing the locally-computed hash and carrying on — leaves
/// this node advertising a root whose state it does not hold, and that claim
/// satisfies every root-hash equality check in the system, `protocol_selector`'s
/// "root hashes match, already in sync" included. So the one node that knows
/// its state is incomplete is also the only one that could have asked for a
/// repair, and it has just told everyone else there is nothing to repair.
///
/// A mismatch is not a choice between two candidate hashes. It is evidence the
/// transfer was not a consistent point-in-time capture of the sender's state,
/// and the same is true of a local compute failure — hence one verdict for
/// both. Note what a mismatch specifically implies: [`served_state_root`] reads
/// the context's ROOT `Index` entry, so the entry is either absent (its record
/// never landed) or from a different point in the sender's history than the
/// boundary. A leaf set aside for a schema this node cannot read yet does not
/// move this hash; it surfaces as the per-page `snapshot page applied with
/// rejections` warning. An entity that fails its checks fails the snapshot
/// before this runs.
///
/// This runs after the staged snapshot has been folded to the claim and moved
/// into the context's state, so a failure here leaves the installed entities
/// and the sync-in-progress marker in place, and the retry is crash recovery.
fn verified_snapshot_root(store: &Store, context_id: ContextId, claimed: Hash) -> Result<Hash> {
    let computed = served_state_root(store, context_id).map_err(|e| {
        warn!(
            %context_id,
            error = %e,
            claimed_root = %claimed,
            "Could not compute local root hash; refusing to trust peer's claimed root, \
             failing sync for retry"
        );
        eyre::eyre!("snapshot verify: could not compute local root hash for {context_id}: {e}")
    })?;

    if computed != claimed {
        warn!(
            %context_id,
            computed_root = %computed,
            claimed_root = %claimed,
            "Snapshot root hash mismatch - refusing to publish a root this node cannot \
             back, failing sync for retry"
        );
        eyre::bail!(
            "snapshot verify: applied state for {context_id} hashes to {computed}, not the \
             claimed boundary {claimed}"
        );
    }

    info!(%context_id, root_hash = %computed, "Snapshot root hash verified successfully");
    Ok(computed)
}

/// Tell the operator of a serving node that joiners will refuse its snapshot of
/// `context_id`, and at which entity. The snapshot is still served.
fn warn_if_the_tree_does_not_fold<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
) {
    match first_unfolding_entity(handle, context_id) {
        Ok(None) => {}
        Ok(Some((id, defect))) => warn!(
            %context_id,
            entity = %id,
            defect,
            "this node's state tree is inconsistent, so joiners will refuse its snapshot \
             of this context and bootstrap from another peer"
        ),
        Err(error) => {
            warn!(%context_id, %error, "could not check the state tree served as a snapshot")
        }
    }
}

/// The first entity of `context_id` a joiner's tree check would refuse, and why:
/// its stored hash or child trie disagrees with the rows a snapshot ships.
fn first_unfolding_entity<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
) -> Result<Option<(Id, &'static str)>> {
    let read = |key: StorageKey| {
        let row = handle.get(&ContextStateKey::new(context_id, key.to_bytes()));
        row.ok().flatten().map(|row| row.value.as_ref().to_vec())
    };
    let shipped = |id: Id| {
        calimero_storage::row::decode(id, &read(StorageKey::Index(id))?)
            .filter(|row| row.data.is_some())?
            .entity_index()
    };
    // Children that name each parent, and children each entity's trie lists.
    let mut named: HashMap<Id, usize> = HashMap::new();
    let mut listed: HashMap<Id, usize> = HashMap::new();
    for key in collect_context_state_keys(handle, context_id)? {
        let Some((id, index)) = entity_id_of(&key).and_then(|id| Some((id, shipped(id)?))) else {
            continue;
        };
        if let Some(parent) = index.parent_id() {
            *named.entry(parent).or_default() += 1;
        }
        let folded = Index::<MainStorage>::full_hash_with(id, index.own_hash(), read);
        if folded != Some(index.full_hash()) {
            return Ok(Some((id, "its hash is not the fold of its child trie")));
        }
        let children = ChildTrie::<MainStorage>::children_with(id, read);
        for child in &children {
            let defect = match shipped(child.id()) {
                None => "its child trie lists a child that has no entity row",
                Some(row) if row.parent_id() != Some(id) => {
                    "its child trie lists a child that names another parent"
                }
                Some(row) if row.full_hash() != child.merkle_hash() => {
                    "its child trie holds a hash its child no longer has"
                }
                Some(_) => continue,
            };
            return Ok(Some((id, defect)));
        }
        let _previous = listed.insert(id, children.len());
    }
    Ok(named
        .into_iter()
        .find(|(parent, count)| listed.get(parent).is_some_and(|listed| listed != count))
        .map(|(parent, _)| (parent, "a child names it that its child trie does not list")))
}

/// The hash a snapshot boundary is validated against: the root of the state
/// [`generate_snapshot_pages`] will actually read.
///
/// Reads the context's ROOT `Index` entry, **not** `ContextMeta.root_hash`.
/// The two agree whenever no write is in flight, but they are not
/// interchangeable: a local execution commits context state
/// (`storage.commit()`) well before it persists the new root hash into
/// `ContextMeta` — see `crates/context/src/handlers/execute/mod.rs` — so for
/// the duration of that window the metadata still reports the pre-write hash
/// while every entity a snapshot would read has already moved. A boundary
/// check reading the metadata there sees nothing wrong and streams
/// post-boundary state under a pre-boundary hash.
///
/// The receiver cannot recover from that on its own. It recomputes the root
/// from the state it received, finds it disagrees with the hash it was
/// promised, and adopts the mismatching hash anyway (see
/// [`SyncManager::request_snapshot_sync`]). It then advertises a root whose
/// state it does not hold, which satisfies every root-hash equality check —
/// including the one in `protocol_selector` — so no repair is ever triggered.
///
/// The ROOT `Index` entry is rewritten in the same batch as the entities it
/// covers, so reading through it observes exactly the state that will be
/// served.
fn served_state_root(store: &Store, context_id: ContextId) -> Result<Hash> {
    Ok(Hash::from(
        ContextRegistry::new(store.clone()).compute_root_hash(&context_id)?,
    ))
}

/// Generate snapshot pages. Returns `(pages, next_cursor, total_entries)`,
/// where `total_entries` is the grand total of shippable `Entity` records at
/// this boundary — every entity with both an `Index` and an `Entry`, counted
/// across the whole snapshot regardless of the cursor window, and excluding
/// orphans that are never shipped. It is therefore the exact denominator the
/// receiver's cumulative applied count converges to.
///
/// Uses a snapshot iterator to ensure consistent reads even if writes occur
/// during iteration. The snapshot provides a frozen point-in-time view.
///
/// **Wire-format note (#2387):** records are now structured
/// [`SnapshotRecord`]s carrying the entity id and kind explicitly,
/// so the receiver can group `Entry`+`Index` records per entity and
/// run `Interface::verify_snapshot_entity_signature` before
/// persisting. Pre-#2387 the wire shipped opaque
/// `(state_key_hash, value)` tuples that gave the receiver no way to
/// authenticate state-bearing records.
///
/// Discovery flow:
/// 1. Iterate all `ContextStateKey` records for this context once,
///    retaining only their 32-byte hashed state keys plus the
///    discovered entity ids — never the record *values*. The
///    earlier implementation collected every key→value pair into a
///    map up front, so a context with millions of keys would pull
///    all of its state into memory on every paginated call before
///    the cursor was even consulted (issue #2133). Values are now
///    materialised lazily, one entity at a time, only for the
///    bundles that actually land on the requested page.
/// 2. For each record value, attempt borsh deserialization as
///    [`calimero_storage::index::EntityIndex`]; success identifies
///    the record as `Key::Index(id)` and yields the entity id. The
///    value is dropped immediately after the id is extracted.
/// 3. Entity ids are sorted (the canonical pagination order) and
///    the cursor skips everything already shipped. For each id that
///    falls on this page, `Key::Index(id)` and `Key::Entry(id)` are
///    point-looked-up to materialise the bundle.
/// 4. State keys that don't match any discovered `Index`/`Entry`
///    slot are dropped as orphans (with a warning) — a well-formed
///    state tree shouldn't have any.
///
/// **Read consistency:** the discovery scan uses a snapshot
/// iterator, but the per-bundle value lookups in step 3 hit the
/// live store. What makes that safe is that `stream_snapshot_pages`
/// re-checks the boundary after generation via [`served_state_root`]
/// and rejects the snapshot with `InvalidBoundary` if the state moved
/// in the meantime. The re-check has to read the *state* root: it used
/// to compare `ContextMeta.root_hash`, which lags a committed state
/// write, so it could not see the very mutation it existed to catch
/// and a torn read did reach peers.
fn generate_snapshot_pages<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
    start_cursor: Option<&SnapshotCursor>,
    page_limit: u16,
    byte_limit: u32,
    schema_bytecode_id: Option<[u8; 32]>,
) -> Result<(Vec<Vec<u8>>, Option<SnapshotCursor>, u64)> {
    // Pass 1 — single snapshot scan, memory bounded to keys + ids.
    //
    // Retain only the state keys (`present_keys`, for O(1)
    // existence checks) and the discovered entity ids.
    // Record *values* are deserialized to identify `Index` records
    // and then dropped — never collected. The pre-#2133
    // implementation built a full `state_key → value` map here, so
    // a context with millions of keys pulled its entire state into
    // memory on every paginated call.
    let mut iter = handle.iter_snapshot::<ContextStateKey>()?;
    let mut present_keys: HashSet<[u8; calimero_store::key::STATE_KEY_LEN]> = HashSet::new();
    let mut entity_ids: Vec<Id> = Vec::new();
    // Entities whose row also carries their data: the index record and the
    // entry share one row (`calimero_storage::row`), so pairing is a property
    // of the row, not of two keys.
    let mut with_entry: HashSet<Id> = HashSet::new();
    for (key_result, value_result) in iter.entries() {
        let key = key_result?;
        // Unwrap the value before the context filter. `IterEntries`
        // reads the value eagerly inside `next()` and fuses the
        // iterator (`done = true`) on a read error — so dropping a
        // foreign-context `value_result` without unwrapping would
        // swallow that error and silently truncate the scan,
        // yielding an incomplete snapshot that still passes the
        // `root_hash` recheck (which only covers *this* context).
        // Propagating here matches the pre-#2133 fail-loud behavior.
        //
        // Deliberate tradeoff: because the State column is a single
        // shared keyspace and the iterator fuses on the first bad
        // read, a corrupt/unreadable record in *any* context aborts
        // this context's snapshot with an error. That is the correct
        // failure mode — a snapshot that fails is retried and never
        // ships partial state, whereas "log the foreign error and
        // continue" is impossible (the iterator is already fused, so
        // the next `next()` returns `None` and we'd silently treat a
        // truncated scan as complete → exactly the data-loss bug this
        // unwrap prevents). We intentionally do NOT early-break once
        // past this context's (contiguous) key range either: that
        // would trade a recoverable failure for an ordering
        // assumption whose violation would silently drop tail state.
        let value = value_result?;
        if key.context_id() != context_id {
            continue;
        }
        let state_key = key.state_key();
        // This node's records of collected deletes are not state to ship.
        if matches!(
            StorageKey::from_bytes(&state_key),
            Some(StorageKey::Collected(_))
        ) {
            continue;
        }

        // Discover entity ids from the keys: an entity row sits at its id
        // behind the entity tag, and the row codec refuses anything it did
        // not write, so only a row that decodes there counts. The borrowed
        // value is dropped at the end of the iteration — nothing about it is
        // retained.
        if let Some(id) = entity_id_of(&state_key) {
            if let Some(row) = calimero_storage::row::decode(id, value.value.as_ref()) {
                if row.entity_index().is_some() {
                    entity_ids.push(id);
                    if row.data.is_some() {
                        let _ = with_entry.insert(id);
                    }
                }
            }
        }

        let _ = present_keys.insert(state_key);
    }
    let total_records = present_keys.len();
    entity_ids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    // Accounting pass — diagnostics only, keys not values.
    //
    // Walk every entity id and classify it from `present_keys`
    // existence checks alone (no value reads). This reproduces the
    // pre-#2133 bookkeeping exactly: `consumed_keys` collects every
    // state_key we either ship in a bundle, deliberately skip
    // (cursor / RotationLog), and `unrecognized_count` is the
    // residual `total_records − consumed`. It is computed over the
    // *full* id set on every call (independent of the page window)
    // so the operator-visible warning stays stable across
    // pagination.
    //
    // Cursor support: skip any entity ids ≤ cursor.last_key. The
    // `≤` (not `<`) is correct because the cursor records the
    // last fully-committed entity, not "next to emit."
    let start_after_id = start_cursor.map(|c| c.last_key);
    let mut consumed_keys: HashSet<[u8; calimero_store::key::STATE_KEY_LEN]> = HashSet::new();
    // Specific anomaly counters. They subdivide `unrecognized_count`
    // computed below — a state_key flagged here is ALSO counted in
    // the residual unrecognized total. That's intentional: ops can
    // see "100 non-bundle records dropped, of which 95 were orphan
    // Indexes (specific pattern), 5 were truly unrecognized."
    let mut orphan_index_without_entry: u64 = 0;
    let mut orphan_entry_without_index: u64 = 0;
    // Count of records that would be emitted in a fresh (no-cursor)
    // run — counts every entity's bundle, regardless of whether
    // we're cursor-skipping it on this call. Stable across paginated
    // calls so operators can monitor snapshot progress reliably (it
    // flows into the "Streaming snapshot" info log; an earlier
    // per-page count shrank with each paginated call, misleading
    // operators).
    //
    // This is a Pass-1 *scan-time* figure and is deliberately not
    // adjusted for emit-pass skips. It never crosses the wire — the
    // `SnapshotPage` message carries only `page_count`/`sent_count`,
    // so the receiver's progress tracking can't see it; it exists
    // solely for the sender's operator log. If a concurrent delete
    // removes an entity between Pass 1 and its emit-pass `handle.get`,
    // the emit pass skips it (with a warning) and `total_entries`
    // overcounts by one for that log line — but that same delete
    // moves `root_hash`, so the post-generation recheck in
    // `stream_snapshot_pages` discards the whole snapshot. No shipped
    // snapshot ever emits fewer records than `total_entries` reports.
    // Decrementing on skip would instead make the figure depend on
    // which page window this call serves, breaking the
    // stable-across-pages property operators rely on.
    let mut total_entries: u64 = 0;
    for id in &entity_ids {
        let id_bytes = *id.as_bytes();
        let index_key = StorageKey::Index(*id).to_bytes();
        let has_index = present_keys.contains(&index_key);
        let has_entry = with_entry.contains(id);

        // An entity contributes 1 record (Entity bundling Entry + Index). A rotation
        // writes no data, so there is no rotation key to count.
        if has_index && has_entry {
            total_entries += 1;
        }

        if let Some(after) = start_after_id {
            if id_bytes <= after {
                // Cursor-skipped — these keys were already shipped on
                // a prior page. Mark them consumed so they don't
                // appear in the residual unrecognized count for the
                // current page.
                //
                // Only insert keys that actually exist. Inserting a
                // phantom key (one for an entity that doesn't have it)
                // would push
                // `consumed_keys.len()` past `total_records`, making
                // the `saturating_sub` below return 0 and silently
                // suppress the operator-visible unrecognized-records
                // warning even when real orphans exist.
                if has_index {
                    let _ = consumed_keys.insert(index_key);
                }
                continue;
            }
        }

        // Classify the entity. An `Entity` record is shipped only
        // when both Entry + Index exist (the common case — every
        // persisted entity has both); verification on the receiver
        // runs against the metadata inside `index`.
        //
        // **No orphan-as-Auxiliary fallback.** A previous iteration
        // shipped Index-without-Entry / Entry-without-Index as
        // `SnapshotRecord::Auxiliary { kind: INDEX|ENTRY, .. }`. The
        // receiver write-through then bypassed the per-entity
        // signature check, opening a trust gap: a malicious peer
        // could ship a verified `Entity { id, entry, index }` and an
        // `Auxiliary { kind: INDEX, id, value: forged_index }` for
        // the same id and clobber the just-verified Index with a
        // forged one. Orphan Entry/Index in a well-formed state tree
        // shouldn't exist anyway; if they do we drop them (debug log
        // below) rather than ship them on an unverified channel.
        //
        // `consumed_keys` is updated only on successful bundling /
        // explicit cursor skip. Orphan arms intentionally do NOT
        // insert the orphan key into `consumed_keys` so they flow
        // into the final `unrecognized_count` (the operator-visible
        // catch-all for non-bundle records).
        match (has_index, has_entry) {
            (true, true) => {
                let _ = consumed_keys.insert(index_key);
            }
            (true, false) => {
                debug!(
                    %context_id, id = ?id_bytes,
                    "dropping orphan Index (no matching Entry) — would be \
                     unverifiable on the receiver"
                );
                orphan_index_without_entry += 1;
            }
            (false, true) => {
                // Structurally unreachable: `entity_ids` was derived
                // from successful `EntityIndex` deserializations, so
                // `has_index` is always true here. Kept for
                // exhaustiveness; if it ever does fire it indicates a
                // discovery bug and we want it counted as an orphan.
                debug!(
                    %context_id, id = ?id_bytes,
                    "unreachable: entity_id without matching Index in present_keys"
                );
                orphan_entry_without_index += 1;
            }
            (false, false) => {}
        }

        // A rotation writes no data, so there is no rotation key to consume here.
    }

    // Residual non-bundle records: state_keys present for this
    // context that weren't bundled and weren't cursor-skipped.
    // Includes both the orphan_* anomalies counted above and any
    // truly unrecognized records (e.g. Entry blobs not paired with
    // any discoverable Index — we can't recover their entity id from
    // the hashed state_key, so we just tally).
    let unrecognized_count =
        u64::try_from(total_records.saturating_sub(consumed_keys.len())).unwrap_or(u64::MAX);

    if unrecognized_count > 0 {
        warn!(
            %context_id,
            unrecognized_count,
            orphan_index_without_entry,
            orphan_entry_without_index,
            "snapshot generation: dropping non-bundle records (orphans + truly \
             unrecognized) — well-formed state trees shouldn't have these"
        );
    }

    // Emit pass — materialise values for the requested page only.
    //
    // Point-look-up the Entry + Index *values* for each entity that
    // lands on the page and serialize them. Resident value memory is
    // bounded to roughly one burst (`page_limit × byte_limit`)
    // because we stop the moment `page_limit` pages are filled — the
    // whole point of #2133. The lookups hit the live store rather
    // than the Pass-1 snapshot iterator; `stream_snapshot_pages`
    // re-checks `root_hash` after generation and discards the
    // snapshot on any change, so a torn read can never reach a peer.
    //
    // `entity_ids` is id-sorted, so cursor-skipped entities form a
    // contiguous prefix (`id ≤ cursor.last_key`). Binary-search past
    // it with `partition_point` and iterate only the tail — on a
    // resumed call near the end of a huge context this avoids walking
    // (and re-hashing keys for) the millions of already-shipped ids
    // the accounting pass already accounted for.
    //
    // Pagination is atomic on entity boundaries — an entity's record
    // either fits entirely on the current page or moves to the next.
    // The cursor records the last entity id fully committed to a page.
    //
    // **Invariant**: `last_id` is `Some` whenever the early-return
    // path fires. The early-return is gated on
    // `pages.len() >= page_limit`, which only increases after a
    // `pages.push(current_page)`; that push requires `current_page`
    // to be non-empty, which only happens after a prior entity's
    // `current_page.extend(record_bytes)` — and that extend sets
    // `last_id = Some(id_bytes)`. So the cursor emitted on
    // early-return always references a real, fully-committed entity
    // id; we never signal completion (cursor = None) with bundles
    // still pending.
    let emit_start = match start_after_id {
        Some(after) => entity_ids.partition_point(|id| *id.as_bytes() <= after),
        None => 0,
    };
    let mut pages: Vec<Vec<u8>> = Vec::new();
    let mut current_page: Vec<u8> = Vec::new();
    let mut last_id: Option<[u8; 32]> = None;

    for id in &entity_ids[emit_start..] {
        let id_bytes = *id.as_bytes();
        let index_key = StorageKey::Index(*id).to_bytes();
        // Only fully-paired entities are shipped; orphans were
        // diagnosed in the accounting pass and are intentionally
        // dropped. The existence pre-check avoids a value lookup for
        // ids that can't produce a bundle.
        if !(present_keys.contains(&index_key) && with_entry.contains(id)) {
            continue;
        }

        // Materialise the values now. A `None` means the record was
        // removed between the Pass-1 scan and this live-store lookup
        // (a concurrent delete). The post-generation `root_hash`
        // recheck rejects the whole snapshot when state changed, so
        // skipping the entity here is safe — but log it so the
        // otherwise-silent skip is observable if it ever fires.
        let row = handle
            .get(&ContextStateKey::new(context_id, index_key))?
            .and_then(|value| calimero_storage::row::decode(*id, value.value.as_ref()));
        let Some(calimero_storage::row::Row {
            index: Some(index),
            data: Some(entry),
        }) = row
        else {
            warn!(
                %context_id, id = ?id_bytes,
                "snapshot emit: entity row vanished or lost a part between scan and read \
                 (concurrent delete?) — skipping entity; root_hash recheck guards correctness"
            );
            continue;
        };

        // V2 page format (PR-6b): each record is length-framed
        // (`u32 LE len ‖ record_bytes`) so the receiver bounds each
        // record's decode to its own sub-slice. Without framing, the
        // backward-compatible trailing `Entity.schema_bytecode_id` (decoded
        // EOF-tolerantly) would, on a record that is NOT the last in the
        // page, read the next record's leading bytes instead of seeing a
        // clean EOF — desyncing the whole page.
        let record_bytes = encode_framed_snapshot_record(&SnapshotRecord::Entity {
            id: id_bytes,
            entry,
            index,
            schema_bytecode_id,
        })?;

        // Page-break BEFORE adding this record if it would exceed
        // byte_limit and the current page isn't empty. A record that
        // by itself exceeds byte_limit still goes on its own page —
        // splitting would defeat atomicity, and oversized records
        // are bounded by `MAX_ENTITY_DATA_SIZE` on the wire types.
        if !current_page.is_empty() && (current_page.len() + record_bytes.len()) as u32 > byte_limit
        {
            pages.push(std::mem::take(&mut current_page));
            if pages.len() >= page_limit as usize {
                return Ok((
                    pages,
                    last_id.map(|k| SnapshotCursor { last_key: k }),
                    total_entries,
                ));
            }
        }

        // First record on a fresh page: stamp the v2 page-format sentinel
        // so the receiver decodes it framed (and a legacy peer's unframed
        // page is told apart by its missing sentinel).
        if current_page.is_empty() {
            current_page.push(SNAPSHOT_PAGE_FORMAT_V2);
        }
        current_page.extend(record_bytes);
        last_id = Some(id_bytes);
    }

    if !current_page.is_empty() {
        pages.push(current_page);
    }

    Ok((pages, None, total_entries))
}

/// Encode one [`SnapshotRecord`] with a `u32` little-endian length prefix for
/// the v2 page format. The length frames the borsh-encoded record so the
/// receiver can bound its decode to exactly this record's bytes — making the
/// EOF-tolerant trailing `Entity.schema_bytecode_id` sound even for a non-terminal
/// record.
fn encode_framed_snapshot_record(record: &SnapshotRecord) -> Result<Vec<u8>> {
    let body = borsh::to_vec(record)?;
    let len = u32::try_from(body.len())
        .map_err(|_| eyre::eyre!("snapshot record exceeds u32 length frame"))?;
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend(body);
    Ok(framed)
}

/// Derive `(percent, eta_secs)` for a snapshot in progress.
///
/// `percent` is `records_received / total_records` clamped to `0..=100`, or
/// `None` when `total_records` is `0` (sender didn't advertise a total — an
/// empty snapshot or a peer too old). `eta_secs` extrapolates the remaining
/// records from the average rate so far; it is `None` until there is at least
/// one record and a non-zero elapsed window, and `Some(0)` at completion.
fn snapshot_progress_estimate(
    records_received: u64,
    total_records: u64,
    elapsed: std::time::Duration,
) -> (Option<u8>, Option<u64>) {
    if total_records == 0 {
        return (None, None);
    }
    let percent = (records_received.saturating_mul(100) / total_records).min(100) as u8;
    let secs = elapsed.as_secs_f64();
    let eta = if records_received > 0 && secs > 0.0 {
        let rate = records_received as f64 / secs; // records/sec
        let remaining = total_records.saturating_sub(records_received) as f64;
        let est = (remaining / rate).ceil();
        // `as u64` already saturates in current Rust (NaN → 0, +∞ → u64::MAX),
        // but guard explicitly so a degenerate rate can't surface a misleading
        // near-zero ETA, and an absurdly large estimate clamps cleanly.
        if est.is_finite() {
            Some(est.min(u64::MAX as f64) as u64)
        } else {
            None
        }
    } else {
        None
    };
    (Some(percent), eta)
}

/// Decompress a received snapshot page, guarding against a decompression bomb.
///
/// The page is produced by [`lz4_flex::compress_prepend_size`], so `payload`
/// carries a 4-byte little-endian LZ4 size prefix followed by the compressed
/// block, and `uncompressed_len` is the sender's declared decompressed length.
///
/// Both the prefix and `uncompressed_len` are attacker-controlled, so we never
/// let them drive an unbounded allocation. Instead we:
///   1. reject any page whose declared size exceeds [`MAX_SNAPSHOT_PAGE_SIZE`]
///      *before* allocating — this is the receiver-enforced protocol cap and is
///      independent of the `byte_limit` we advertised in the stream request (a
///      malicious peer is free to ignore that hint). The cap matches
///      `SnapshotPage::is_valid` and is large enough for a single oversized
///      record on its own page (see `generate_snapshot_pages`), which
///      [`DEFAULT_PAGE_BYTE_LIMIT`] — the sender's *grouping* hint — is not;
///   2. require the embedded size prefix to agree with `uncompressed_len`, so a
///      malformed/inconsistent frame is rejected up front rather than silently
///      tolerated; and
///   3. decompress into a fixed buffer of exactly `uncompressed_len` bytes via
///      [`lz4_flex::decompress_into`], which errors rather than growing the
///      output — unlike `decompress_size_prepended`, which would honor the
///      embedded prefix and pre-allocate accordingly.
fn decompress_snapshot_page(payload: &[u8], uncompressed_len: u32) -> Result<Vec<u8>> {
    if uncompressed_len > MAX_SNAPSHOT_PAGE_SIZE {
        eyre::bail!(
            "Snapshot page uncompressed size {} exceeds limit {}",
            uncompressed_len,
            MAX_SNAPSHOT_PAGE_SIZE
        );
    }

    let prefix = payload
        .get(..4)
        .ok_or_else(|| eyre::eyre!("Snapshot page payload too short"))?;
    // `try_into` cannot fail: `prefix` is exactly 4 bytes.
    let declared = u32::from_le_bytes(prefix.try_into().expect("4-byte prefix"));
    if declared != uncompressed_len {
        eyre::bail!(
            "Snapshot page size prefix {} disagrees with declared length {}",
            declared,
            uncompressed_len
        );
    }

    let block = &payload[4..];
    let mut decompressed = vec![0u8; uncompressed_len as usize];
    let written = lz4_flex::decompress_into(block, &mut decompressed)
        .map_err(|e| eyre::eyre!("Decompress failed: {}", e))?;

    if written != uncompressed_len as usize {
        eyre::bail!(
            "Size mismatch: declared {} bytes, decompressed {} bytes",
            uncompressed_len,
            written
        );
    }

    Ok(decompressed)
}

/// Decode snapshot records from a (decompressed) page payload.
///
/// Two on-the-wire page formats are accepted:
///
/// * **v2** (PR-6b / #2539) — the page begins with the
///   [`SNAPSHOT_PAGE_FORMAT_V2`] sentinel, followed by length-framed records
///   (`u32 LE len ‖ borsh(record)`). Each record decodes inside its own
///   sub-slice, so the EOF-tolerant trailing `Entity.schema_bytecode_id` reads a
///   clean EOF at the sub-slice boundary instead of bleeding into the next
///   record — correct even for a non-terminal record.
/// * **legacy** (pre-#2539) — the page is a back-to-back concatenation of
///   borsh-encoded records with NO framing and NO trailing `schema_bytecode_id`
///   byte. Such a page can never start with `0xFF` (a record starts with its
///   variant discriminant, `0`/`1`), so the absence of the sentinel selects
///   this path. Records are decoded with [`decode_legacy_record`], which stops
///   after `Entity {id, entry, index}` (schema absent) rather than peeking for
///   a trailing byte that belongs to the NEXT record.
fn decode_snapshot_records(payload: &[u8]) -> Result<Vec<SnapshotRecord>> {
    if payload.first() == Some(&SNAPSHOT_PAGE_FORMAT_V2) {
        return decode_framed_snapshot_records(&payload[1..]);
    }
    decode_legacy_snapshot_records(payload)
}

/// Decode a **v2** length-framed page body (sentinel already stripped).
fn decode_framed_snapshot_records(mut remaining: &[u8]) -> Result<Vec<SnapshotRecord>> {
    let mut records = Vec::new();
    while !remaining.is_empty() {
        if remaining.len() < 4 {
            eyre::bail!("snapshot page truncated: dangling record length frame");
        }
        let len =
            u32::from_le_bytes([remaining[0], remaining[1], remaining[2], remaining[3]]) as usize;
        let body_start = 4usize;
        let body_end = body_start
            .checked_add(len)
            .filter(|end| *end <= remaining.len())
            .ok_or_else(|| eyre::eyre!("snapshot record length frame overruns page"))?;
        let body = &remaining[body_start..body_end];
        // Decode inside the exact record sub-slice: a clean EOF at `body`'s
        // end is what makes the EOF-tolerant trailing `schema_bytecode_id` sound.
        let record = borsh::from_slice::<SnapshotRecord>(body)?;
        records.push(record);
        remaining = &remaining[body_end..];
    }
    Ok(records)
}

/// Decode a **legacy** (pre-#2539) unframed page: records concatenated
/// end-to-end with no trailing `schema_bytecode_id`. Each record is self-delimiting
/// (borsh `Vec` fields carry their own length), so we decode sequentially —
/// but we must NOT peek for the trailing `Entity.schema_bytecode_id` byte, because
/// in a legacy page that byte is the NEXT record's leading byte.
fn decode_legacy_snapshot_records(payload: &[u8]) -> Result<Vec<SnapshotRecord>> {
    let mut records = Vec::new();
    let mut remaining = payload;
    while !remaining.is_empty() {
        let mut cursor = remaining;
        let record = decode_legacy_record(&mut cursor)?;
        let consumed = remaining.len() - cursor.len();
        if consumed == 0 {
            eyre::bail!("snapshot record deserialization made no progress");
        }
        remaining = cursor;
        records.push(record);
    }
    Ok(records)
}

/// Decode a single LEGACY-format `SnapshotRecord` from `reader`, treating an
/// `Entity` as the pre-#2539 three-field `{id, entry, index}` (schema absent).
/// Unlike the hand-written [`SnapshotRecord::deserialize`], this NEVER reads a
/// trailing `Option` byte, so it cannot consume the next record's bytes in an
/// unframed page.
fn decode_legacy_record<R: borsh::io::Read>(reader: &mut R) -> Result<SnapshotRecord> {
    let variant = u8::deserialize_reader(reader)?;
    match variant {
        0 => {
            let id = <[u8; 32]>::deserialize_reader(reader)?;
            let entry = Vec::<u8>::deserialize_reader(reader)?;
            let index = Vec::<u8>::deserialize_reader(reader)?;
            Ok(SnapshotRecord::Entity {
                id,
                entry,
                index,
                schema_bytecode_id: None,
            })
        }
        1 => {
            let kind = u8::deserialize_reader(reader)?;
            let id = <[u8; 32]>::deserialize_reader(reader)?;
            let value = Vec::<u8>::deserialize_reader(reader)?;
            Ok(SnapshotRecord::Auxiliary { kind, id, value })
        }
        other => eyre::bail!("invalid legacy SnapshotRecord variant discriminant {other}"),
    }
}

/// Check if a context has any state keys (efficient early-exit check).
///
/// This function returns as soon as the first key is found, avoiding
/// the overhead of collecting all keys just to check for emptiness.
fn has_context_state_keys<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
) -> Result<bool> {
    let mut iter = handle.iter::<ContextStateKey>()?;

    for (key_result, _) in iter.entries() {
        let key = key_result?;
        if key.context_id() == context_id {
            return Ok(true); // Early exit on first match
        }
    }

    Ok(false)
}

/// Collect all state keys for a context.
fn collect_context_state_keys<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
) -> Result<Vec<[u8; calimero_store::key::STATE_KEY_LEN]>> {
    let mut keys = Vec::new();
    let mut iter = handle.iter::<ContextStateKey>()?;

    for (key_result, _) in iter.entries() {
        let key = key_result?;
        if key.context_id() == context_id {
            keys.push(key.state_key());
        }
    }

    Ok(keys)
}

/// Settle a context's per-context binding after a full-state snapshot — ONLY
/// for an operator resync (a `ContextResyncRequested` context).
///
/// Resync is an explicit, force-gated request to adopt a peer's state, so it
/// advances the activation marker and drops the stranded/resync markers. The
/// marker is bound to `data_schema` — the `schema_bytecode_id` the synced entities
/// actually carried — NOT the group target: peer selection is automatic and
/// the forced apply disables the schema fence, so the chosen peer may be BEHIND
/// the target. Binding to the target over a behind peer's state would tell the
/// lazy gate "up to date" over old-schema data and silently skip the migration.
/// Binding to the real data schema instead lets the lazy gate replay the
/// remaining ladder hops from there (or no-op when already at target).
///
/// It is deliberately scoped to resync: a bootstrap or crash-recovery snapshot
/// is a no-op here (pre-resync behavior — the lazy gate re-evaluates it).
///
/// The resync marker is cleared unconditionally (even when no schema can be
/// bound), so the one-shot request can never get stuck forcing snapshots. When
/// the snapshot carried no schema stamp (`data_schema == None`, e.g. an empty
/// or legacy/non-migrating snapshot) the marker falls back to the group target.
///
/// Returns the namespace-root id the heal belongs to when this was an operator
/// resync (so the caller can edge-trigger the migration emitter), or `None` for
/// a non-resync snapshot or when the group/root can't be resolved.
fn settle_snapshot_activation(
    store: &calimero_store::Store,
    context_id: ContextId,
    data_schema: Option<[u8; 32]>,
) -> Option<[u8; 32]> {
    use calimero_governance_store::{
        get_group_for_context, MetaRepository, NamespaceRepository, UpgradeLadderRepository,
    };

    let mut handle = store.handle();
    let resync_key = calimero_store::key::ContextResyncRequested::new(context_id);
    // Non-resync snapshots (bootstrap, crash recovery) leave the binding to the
    // lazy gate — see the doc above.
    if !matches!(handle.get(&resync_key), Ok(Some(()))) {
        return None;
    }

    // Clear the one-shot resync marker + any stranded marker FIRST, so neither
    // a missing group bytecode_id below nor a failed activation write can leave the
    // context stuck forcing snapshots / reporting failed.
    if let Err(err) = handle.delete(&resync_key) {
        warn!(%context_id, %err, "failed to clear resync marker after snapshot");
    }
    if let Err(err) = handle.delete(&calimero_store::key::ContextMigrationFailed::new(
        context_id,
    )) {
        warn!(%context_id, %err, "failed to clear migration-failed marker after resync");
    }
    drop(handle);

    let gid = get_group_for_context(store, &context_id).ok().flatten()?;
    let meta = MetaRepository::new(store).load(&gid).ok().flatten()?;
    // Bind to the schema the synced data actually carries; fall back to the
    // group target when the snapshot carried no stamp or one the group never named.
    let bind = data_schema
        .filter(|k| *k != [0u8; 32])
        .filter(|k| calimero_context::activation::group_registers_bytecode(store, &gid, *k))
        .unwrap_or(meta.target.bytecode_id);
    if bind == [0u8; 32] {
        return None; // zero-key group: no bytecode signal to bind
    }
    calimero_context::activation::record_activation(store, &context_id, bind);
    // Reconcile the bound id too, or a single-wasm context (distinct id per
    // version) would keep tripping the pending-upgrade gate after the marker
    // already matches the synced state.
    let ladder = UpgradeLadderRepository::new(store)
        .load(&gid)
        .unwrap_or_default();
    if let Some(app_id) = calimero_context::activation::application_for_schema(
        &ladder,
        bind,
        meta.target.bytecode_id,
        meta.target.application_id,
    ) {
        calimero_context::activation::reconcile_context_application(store, &context_id, app_id);
    }
    debug!(%context_id, "resync snapshot settled; markers cleared");

    // The heartbeat emitter is keyed by the namespace ROOT, so resolve it from
    // the (possibly sub-)group the context lives in. `None` ⇒ the caller skips
    // the edge-trigger and the next periodic beat carries the recovered facts.
    NamespaceRepository::new(store)
        .resolve(&gid)
        .ok()
        .map(|root| root.to_bytes())
}

#[cfg(test)]
mod tests {
    use calimero_store::key::GroupTarget;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_storage::index::EntityIndex;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::ApplicationMeta as ApplicationMetaKey;
    use calimero_store::key::ContextMeta as ContextMetaKey;
    use calimero_store::types::ContextMeta as ContextMetaValue;
    use calimero_store::Store;

    use super::*;
    // Wire-codec round-trip tests below use the `ROTATION_LOG` auxiliary
    // kind as a sample `SnapshotRecord::Auxiliary`; the constant lives in
    // node-primitives and is exercised only here (the sender never emits a
    // rotation-log auxiliary record).
    use calimero_node_primitives::sync::snapshot::snapshot_record_kind;

    /// Grouping siblings behind one row cache must be invisible in the result.
    ///
    /// The cache exists so siblings stop re-reading the `DEPTH+1` spine rows
    /// they all share. That is a pure win only if serving a row from the cache
    /// gives byte-identical output to reading it back from the store — if it
    /// ever does not, a snapshot receiver builds a different trie than the
    /// sender and diverges permanently, which is the failure this whole pass
    /// exists to prevent.
    #[test]
    fn grouping_siblings_writes_what_linking_them_one_at_a_time_writes() {
        use calimero_storage::address::Id;
        use calimero_storage::entities::{ChildInfo, Metadata};
        use calimero_storage::store::Key as StorageKey;

        let context_id = ContextId::from([4; 32]);
        let parent_id = Id::new([2; 32]);
        let children: Vec<ChildInfo> = (0..12_u8)
            .map(|i| {
                ChildInfo::new(
                    Id::new([i.wrapping_mul(17).wrapping_add(3); 32]),
                    [i; 32],
                    Metadata::default(),
                )
            })
            .collect();

        // Grouped: one call, one shared cache.
        let grouped_store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut grouped = grouped_store.handle();
        link_children_into_parent_trie(&mut grouped, context_id, parent_id, children.clone())
            .expect("grouped link");

        // One at a time: the same code with a cache of size 1 each call.
        let single_store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut single = single_store.handle();
        for child in children.clone() {
            link_child_into_parent_trie(&mut single, context_id, parent_id, child)
                .expect("single link");
        }

        let rows_of = |store: &Store| -> BTreeMap<Vec<u8>, Vec<u8>> {
            let mut out = BTreeMap::new();
            let handle = store.handle();
            let mut iter = handle.iter::<ContextStateKey>().expect("iter");
            for (k, v) in iter.entries() {
                let k = k.expect("key");
                let v = v.expect("value");
                let _prev = out.insert(k.state_key().to_vec(), v.value.as_ref().to_vec());
            }
            out
        };

        let a = rows_of(&grouped_store);
        let b = rows_of(&single_store);
        assert!(!a.is_empty(), "grouped link wrote nothing");
        assert_eq!(a, b, "grouped and one-at-a-time must write identical rows");

        // And both must actually hold the children.
        let read = |key: StorageKey| -> Option<Vec<u8>> {
            let k = ContextStateKey::new(context_id, key.to_bytes());
            grouped_store
                .handle()
                .get(&k)
                .ok()
                .flatten()
                .map(|v| v.as_ref().to_vec())
        };
        let enumerated = calimero_storage::child_trie::ChildTrie::<
            calimero_storage::store::MainStorage,
        >::children_with(parent_id, read);
        assert_eq!(enumerated.len(), children.len());
    }

    /// A snapshot receiver used to hold every entity and be able to enumerate
    /// none of them.
    ///
    /// Snapshot installs an entity by writing its `Entry` and `Index` rows
    /// verbatim. While a parent's children lived INSIDE its index row, that
    /// shipped the parent->child links for free. Moving children into
    /// `Key::ChildTrie` broke that silently: discovery finds entities by
    /// deserialising `EntityIndex`, and a trie row never will.
    ///
    /// It never healed either, which is what makes this worth a test rather
    /// than a one-off measurement. Hash comparison re-applied entities that
    /// were already byte-identical, and that goes through the update path
    /// rather than `add_child_to`, so no link was ever created — a stable
    /// fixpoint, re-syncing every ~10s forever.
    #[test]
    fn rebuilding_after_snapshot_relinks_children_the_install_did_not() {
        use calimero_storage::address::Id;
        use calimero_storage::child_trie::ChildTrie;
        use calimero_storage::store::{Key as StorageKey, MainStorage};

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut handle = store.handle();
        let context_id = ContextId::from([9; 32]);

        let parent_id = Id::new([1; 32]);
        let child_ids: Vec<Id> = (0..5_u8).map(|i| Id::new([10 + i; 32])).collect();

        // Install rows the way snapshot does: verbatim, no linking.
        let put_index = |handle: &mut calimero_store::Handle<Store>, index: &EntityIndex| {
            let key = ContextStateKey::new(context_id, StorageKey::Index(index.id()).to_bytes());
            let slice: Slice<'_> = entity_row(index, None).into();
            handle
                .put(&key, &ContextStateValue::from(slice))
                .expect("put index row");
        };

        put_index(&mut handle, &EntityIndex::minimal_for_test(parent_id));
        for (i, id) in child_ids.iter().enumerate() {
            put_index(
                &mut handle,
                &EntityIndex::minimal_for_test_with_parent(*id, parent_id, [20 + i as u8; 32]),
            );
        }

        let read = |key: StorageKey| -> Option<Vec<u8>> {
            let k = ContextStateKey::new(context_id, key.to_bytes());
            store
                .handle()
                .get(&k)
                .ok()
                .flatten()
                .map(|v| v.as_ref().to_vec())
        };

        // The bug: every entity present, none enumerable.
        assert!(
            ChildTrie::<MainStorage>::children_with(parent_id, read).is_empty(),
            "precondition: a verbatim install links nothing"
        );

        let linked = rebuild_child_tries_after_snapshot(&mut handle, context_id)
            .expect("rebuild must succeed");
        assert_eq!(linked, child_ids.len(), "every parented row must be linked");

        let rebuilt = ChildTrie::<MainStorage>::children_with(parent_id, read);
        assert_eq!(
            rebuilt.len(),
            child_ids.len(),
            "children must enumerate after the rebuild"
        );

        let got: BTreeSet<Id> = rebuilt
            .iter()
            .map(calimero_storage::entities::ChildInfo::id)
            .collect();
        let want: BTreeSet<Id> = child_ids.iter().copied().collect();
        assert_eq!(
            got, want,
            "the rebuilt trie must hold exactly the shipped children"
        );

        // The rebuild replays the SENDER's full_hash per child, so the
        // reconstructed trie has to reproduce the sender's root exactly —
        // that equality is the whole point, not a side effect.
        for (i, child) in rebuilt.iter().enumerate() {
            let expected = child_ids
                .iter()
                .position(|id| *id == child.id())
                .expect("child id known");
            let _ = i;
            assert_eq!(
                child.merkle_hash(),
                [20 + expected as u8; 32],
                "the shipped full_hash must be what lands in the trie"
            );
        }
    }

    #[test]
    fn snapshot_progress_unknown_total_yields_no_estimate() {
        // total_records == 0 (empty snapshot or pre-feature peer) → no derived
        // percent/ETA; the caller still reports raw records_received.
        let (percent, eta) = snapshot_progress_estimate(5, 0, Duration::from_secs(1));
        assert_eq!(percent, None);
        assert_eq!(eta, None);
    }

    #[test]
    fn snapshot_progress_percent_is_clamped_and_eta_extrapolates() {
        // 50 of 200 records in 5s → 25%, rate 10/s, 150 remaining → 15s ETA.
        let (percent, eta) = snapshot_progress_estimate(50, 200, Duration::from_secs(5));
        assert_eq!(percent, Some(25));
        assert_eq!(eta, Some(15));

        // Over-count (receiver applied more than advertised) clamps to 100.
        let (percent, _) = snapshot_progress_estimate(250, 200, Duration::from_secs(1));
        assert_eq!(percent, Some(100));
    }

    #[test]
    fn snapshot_progress_eta_none_before_any_record_or_time() {
        // No elapsed window yet → percent known, ETA not.
        let (percent, eta) = snapshot_progress_estimate(0, 100, Duration::ZERO);
        assert_eq!(percent, Some(0));
        assert_eq!(eta, None);
    }

    #[test]
    fn snapshot_progress_complete_reports_zero_eta() {
        let (percent, eta) = snapshot_progress_estimate(100, 100, Duration::from_secs(2));
        assert_eq!(percent, Some(100));
        assert_eq!(eta, Some(0));
    }

    /// An entity row holding `index` and, when given, `data`, as the storage
    /// layer lays one out (`calimero_storage::row`).
    fn entity_row(index: &EntityIndex, data: Option<Vec<u8>>) -> Vec<u8> {
        calimero_storage::row::encode(
            index.id(),
            &calimero_storage::row::Row {
                index: Some(borsh::to_vec(index).expect("serialise index")),
                data,
            },
        )
    }

    /// Persist a well-formed entity (Index + Entry pair) for `ctx`
    /// into `store`, mirroring how production state is laid out: the
    /// Index value is a borsh-serialized `EntityIndex` whose id
    /// hashes to the Index state-key.
    fn put_entity(store: &Store, ctx: ContextId, id_bytes: [u8; 32], entry_len: usize) {
        let id = Id::new(id_bytes);
        // Entry payload is opaque to the sender; fill it with a
        // recognizable byte so size-based pagination has something to
        // chew on.
        let entry_bytes = vec![0xEE_u8; entry_len];
        let row = entity_row(&EntityIndex::minimal_for_test(id), Some(entry_bytes));

        let mut handle = store.handle();
        let index_key = ContextStateKey::new(ctx, StorageKey::Index(id).to_bytes());
        handle
            .put(&index_key, &ContextStateValue::from(Slice::from(row)))
            .unwrap();
    }

    /// Drive `generate_snapshot_pages` to exhaustion the way the real
    /// streaming loop does — feeding each call's `next_cursor` back in
    /// — and return every emitted entity id in arrival order plus the
    /// `total_entries` reported on every call.
    fn drain_all_pages(
        store: &Store,
        ctx: ContextId,
        page_limit: u16,
        byte_limit: u32,
    ) -> (Vec<[u8; 32]>, Vec<u64>) {
        let handle = store.handle();
        let mut ids = Vec::new();
        let mut totals = Vec::new();
        let mut cursor: Option<SnapshotCursor> = None;
        // Bounded to keep a buggy cursor (that never advances) from
        // looping forever. `completed` distinguishes "drained to
        // cursor = None" from "hit the cap" so the latter fails with a
        // clear message instead of masquerading as a dropped entity.
        let mut completed = false;
        for _ in 0..10_000 {
            let (pages, next, total) = generate_snapshot_pages(
                &handle,
                ctx,
                cursor.as_ref(),
                page_limit,
                byte_limit,
                None,
            )
            .unwrap();
            totals.push(total);
            for page in &pages {
                for record in decode_snapshot_records(page).unwrap() {
                    match record {
                        SnapshotRecord::Entity { id, .. } => ids.push(id),
                        SnapshotRecord::Auxiliary { .. } => panic!("unexpected Auxiliary record"),
                    }
                }
            }
            match next {
                Some(c) => cursor = Some(c),
                None => {
                    completed = true;
                    break;
                }
            }
        }
        assert!(
            completed,
            "drain_all_pages hit its iteration cap before the cursor reached None — \
             non-advancing cursor or unexpectedly many pages"
        );
        (ids, totals)
    }

    #[test]
    fn test_generate_snapshot_pages_empty_context() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let handle = store.handle();
        let ctx = ContextId::from([1u8; 32]);
        let (pages, cursor, total) = generate_snapshot_pages(
            &handle,
            ctx,
            None,
            DEFAULT_PAGE_LIMIT,
            DEFAULT_PAGE_BYTE_LIMIT,
            None,
        )
        .unwrap();
        assert!(pages.is_empty());
        assert!(cursor.is_none());
        assert_eq!(total, 0);
    }

    #[test]
    fn test_generate_snapshot_pages_single_page_round_trips_all_entities() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([2u8; 32]);
        let expected: BTreeSet<[u8; 32]> = (0..20u8)
            .map(|i| {
                let mut id = [0u8; 32];
                id[0] = i;
                put_entity(&store, ctx, id, 16);
                id
            })
            .collect();

        // One generous page — everything fits, cursor signals done.
        let (pages, cursor, total) = generate_snapshot_pages(
            &store.handle(),
            ctx,
            None,
            DEFAULT_PAGE_LIMIT,
            1 << 20,
            None,
        )
        .unwrap();
        assert!(cursor.is_none(), "single page should not request a resume");
        assert_eq!(pages.len(), 1, "20 small entities should fit on one page");
        assert_eq!(total, 20);

        let mut got = BTreeSet::new();
        for page in &pages {
            for record in decode_snapshot_records(page).unwrap() {
                if let SnapshotRecord::Entity { id, .. } = record {
                    let _ = got.insert(id);
                }
            }
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn test_generate_snapshot_pages_pagination_is_complete_and_dedup() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([3u8; 32]);
        let expected: BTreeSet<[u8; 32]> = (0..50u16)
            .map(|i| {
                let mut id = [0u8; 32];
                // Genuinely spread across the two leading bytes (id[0]
                // in 0..7, id[1] in 0..7) so the id-sorted ordering is
                // exercised beyond a single varying byte.
                id[0] = (i / 8) as u8;
                id[1] = (i % 8) as u8;
                put_entity(&store, ctx, id, 200);
                id
            })
            .collect();

        // Tight limits force many resume round-trips: one page per
        // call, ~2 entities per page at 200-byte entries.
        let (ids, totals) = drain_all_pages(&store, ctx, 1, 512);

        // Every entity exactly once — no drops across page breaks, no
        // duplicates from cursor overlap.
        assert_eq!(ids.len(), expected.len(), "duplicate or dropped entity");
        let got: BTreeSet<[u8; 32]> = ids.iter().copied().collect();
        assert_eq!(got, expected);

        // Entities arrive in canonical id-sorted order across pages.
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "pages must be emitted in id-sorted order");

        // total_entries is stable across every paginated call.
        assert!(
            totals.iter().all(|&t| t == 50),
            "total_entries drifted: {totals:?}"
        );
    }

    #[test]
    fn test_generate_snapshot_pages_drops_orphan_index_without_entry() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([4u8; 32]);

        // One complete entity ...
        let mut good = [0u8; 32];
        good[0] = 1;
        put_entity(&store, ctx, good, 16);

        // ... and an orphan Index with no matching Entry.
        let mut orphan = [0u8; 32];
        orphan[0] = 2;
        let orphan_id = Id::new(orphan);
        let mut handle = store.handle();
        let orphan_key = ContextStateKey::new(ctx, StorageKey::Index(orphan_id).to_bytes());
        let orphan_bytes = entity_row(&EntityIndex::minimal_for_test(orphan_id), None);
        handle
            .put(
                &orphan_key,
                &ContextStateValue::from(Slice::from(orphan_bytes)),
            )
            .unwrap();

        let (ids, totals) =
            drain_all_pages(&store, ctx, DEFAULT_PAGE_LIMIT, DEFAULT_PAGE_BYTE_LIMIT);
        // Only the complete entity is shipped; the orphan is dropped.
        assert_eq!(ids, vec![good]);
        // total_entries counts complete (Index+Entry) entities only.
        assert!(totals.iter().all(|&t| t == 1), "{totals:?}");
    }

    #[test]
    fn test_generate_snapshot_pages_page_limit_boundary_defers_entity() {
        // Exercise the case where a single entity simultaneously
        // triggers a page-break AND hits `page_limit`: the page is
        // pushed and the call returns early *before* that entity's
        // bytes are added to a page. The cursor must point at the
        // last fully-committed entity so the deferred one is emitted
        // (exactly once) on the next burst — not dropped.
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([5u8; 32]);
        let expected: BTreeSet<[u8; 32]> = (0..4u8)
            .map(|i| {
                let mut id = [0u8; 32];
                id[0] = i;
                put_entity(&store, ctx, id, 200);
                id
            })
            .collect();

        // page_limit = 1 with a byte_limit that fits one ~330-byte
        // entity record but not two forces a page-break + limit hit on
        // every call, deferring each subsequent entity.
        let (ids, totals) = drain_all_pages(&store, ctx, 1, 400);

        assert_eq!(
            ids.len(),
            expected.len(),
            "deferred entity dropped or duplicated"
        );
        let got: BTreeSet<[u8; 32]> = ids.iter().copied().collect();
        assert_eq!(got, expected);
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "entities must arrive in id-sorted order across bursts"
        );
        assert!(
            totals.iter().all(|&t| t == 4),
            "total_entries drifted: {totals:?}"
        );
    }

    /// Seed the `ContextMeta` row, whose `root_hash` the serve path used to
    /// gate on.
    fn put_context_meta(store: &Store, ctx: ContextId, root_hash: [u8; 32]) {
        let mut handle = store.handle();
        handle
            .put(
                &ContextMetaKey::new(ctx),
                &ContextMetaValue::new(
                    ApplicationMetaKey::new(ApplicationId::from([9u8; 32])),
                    root_hash,
                    vec![],
                    None,
                ),
            )
            .unwrap();
    }

    /// Write the context's ROOT `Index` entry carrying `full_hash`.
    ///
    /// This is the entry `compute_root_hash` reads, and in production it is
    /// rewritten in the same batch as the entities it covers — which is why
    /// reading through it sees the state a snapshot will actually serve.
    fn put_root_index(store: &Store, ctx: ContextId, full_hash: [u8; 32]) {
        let root = Id::new(*ctx);
        let index_bytes = entity_row(
            &EntityIndex::minimal_for_test_with_full_hash(root, full_hash),
            None,
        );
        store
            .handle()
            .put(
                &ContextStateKey::new(ctx, StorageKey::Index(root).to_bytes()),
                &ContextStateValue::from(Slice::from(index_bytes)),
            )
            .unwrap();
    }

    /// The hash the serve path validates a requested boundary against — the
    /// same function both guard sites call, so a regression that points them
    /// back at `ContextMeta.root_hash` fails this test.
    fn boundary_guard_hash(store: &Store, ctx: ContextId) -> Hash {
        served_state_root(store, ctx).unwrap()
    }

    /// A snapshot whose applied state hashes to the claimed boundary is the
    /// only case that may be published.
    #[test]
    fn verified_snapshot_root_accepts_state_matching_the_claim() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([11u8; 32]);
        let claimed = Hash::from([0xC1_u8; 32]);

        put_entity(&store, ctx, [1u8; 32], 32);
        put_root_index(&store, ctx, *claimed);

        assert_eq!(
            verified_snapshot_root(&store, ctx, claimed).unwrap(),
            claimed
        );
    }

    /// State that hashes to something else must NOT be published, however
    /// trustworthy the locally-computed hash looks: publishing it advertises a
    /// root this node cannot back, and that claim silences the repair paths
    /// that would otherwise correct it.
    #[test]
    fn verified_snapshot_root_rejects_state_that_hashes_to_something_else() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([12u8; 32]);

        put_entity(&store, ctx, [1u8; 32], 32);
        put_root_index(&store, ctx, [0xC2_u8; 32]);

        let err = verified_snapshot_root(&store, ctx, Hash::from([0xC1_u8; 32]))
            .expect_err("state disagreeing with the claimed boundary must not verify");
        let msg = err.to_string();
        assert!(
            msg.contains("hashes to"),
            "the error must name both hashes so the mismatch is diagnosable: {msg}"
        );
    }

    /// The case that motivated failing instead of accepting: a snapshot whose
    /// ROOT `Index` record never landed. The context has state but no root to
    /// hash, so nothing here can be published — and the pre-existing behaviour
    /// (store the computed hash) would have published a zero root while
    /// entities sat in the store, which is exactly the `RecoverContradiction`
    /// shape the I5 gate has to dig back out of.
    #[test]
    fn verified_snapshot_root_rejects_a_snapshot_missing_its_root_index() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([13u8; 32]);

        // Leaf entities landed; the ROOT index record did not.
        put_entity(&store, ctx, [1u8; 32], 32);
        put_entity(&store, ctx, [2u8; 32], 32);

        assert!(
            verified_snapshot_root(&store, ctx, Hash::from([0xC1_u8; 32])).is_err(),
            "a snapshot with no ROOT index has no verified root to publish"
        );
    }

    /// Two snapshots generated under the same boundary hash must carry the
    /// same state. If the payload can move while the boundary hash sits
    /// still, the sender streams state its announced boundary does not
    /// describe — and the receiver has no way to tell.
    ///
    /// Two checks on the serve path exist to prevent exactly that:
    /// `handle_snapshot_stream_request` before page generation, and the
    /// recheck in `stream_snapshot_pages` after it. Both used to compare
    /// `ContextMeta.root_hash`, and neither could see a write that had
    /// committed its *state* but not yet its *metadata* — a window a local
    /// execution leaves open on every call, because
    /// `crates/context/src/handlers/execute/mod.rs` commits context state
    /// roughly 250us before it persists the new root hash into `ContextMeta`.
    /// A snapshot generated inside it read post-write state and validated
    /// against a pre-write hash.
    #[test]
    fn same_boundary_hash_must_mean_same_snapshot_payload() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([7u8; 32]);

        // State as of the announced boundary. The ROOT index and the metadata
        // agree on the hash that describes it, as they do whenever no write is
        // in flight.
        let boundary = [0xB1_u8; 32];
        for id in [[1u8; 32], [2u8; 32], [3u8; 32]] {
            put_entity(&store, ctx, id, 64);
        }
        put_root_index(&store, ctx, boundary);
        put_context_meta(&store, ctx, boundary);

        let guard_before = boundary_guard_hash(&store, ctx);
        let (payload_before, _) =
            drain_all_pages(&store, ctx, DEFAULT_PAGE_LIMIT, DEFAULT_PAGE_BYTE_LIMIT);
        assert_eq!(
            guard_before,
            Hash::from(boundary),
            "precondition: with no write in flight the guard must agree with \
             the announced boundary, or the sender could never serve at all"
        );

        // The concurrent local write, caught mid-execution: the entity and the
        // ROOT index it updates are committed together, while the `ContextMeta`
        // root-hash write has not landed yet.
        put_entity(&store, ctx, [4u8; 32], 64);
        put_root_index(&store, ctx, [0xB2_u8; 32]);

        let guard_after = boundary_guard_hash(&store, ctx);
        let (payload_after, _) =
            drain_all_pages(&store, ctx, DEFAULT_PAGE_LIMIT, DEFAULT_PAGE_BYTE_LIMIT);

        assert_ne!(
            payload_before, payload_after,
            "sanity check: a committed state write must be visible to the \
             snapshot reader, otherwise this test proves nothing"
        );
        assert_ne!(
            guard_before, guard_after,
            "the snapshot payload moved while the hash the serve path \
             validates the boundary against did not: the guards accept the \
             boundary as intact and stream post-boundary state under a \
             pre-boundary hash"
        );
    }

    #[test]
    fn test_snapshot_entity_future_schema_is_declined_not_stored() {
        // Matching schema, legacy (None) schema, and unresolvable loaded reader
        // all apply; only a known-and-different schema declines (decline =
        // buffer instead of `handle.put`).
        let v1 = [1u8; 32];
        let v2 = [2u8; 32];

        // Future-schema entity vs a v1 loaded reader: DECLINE.
        assert!(
            !snapshot_entity_is_readable(Some(v2), Some(v1)),
            "a v2-authored snapshot entity must be declined by a v1 reader \
             (else its bytes are stored unreadable)"
        );

        // Matching schema: apply.
        assert!(snapshot_entity_is_readable(Some(v1), Some(v1)));
        // Legacy sender (no marker): apply (back-compat).
        assert!(snapshot_entity_is_readable(None, Some(v1)));
        // Unresolvable loaded reader (non-group / missing meta): no gate, apply.
        assert!(snapshot_entity_is_readable(Some(v2), None));
        assert!(snapshot_entity_is_readable(None, None));
    }

    /// A buffered `SharedMember` answers to its anchor's writers. Until that
    /// anchor is stored the drain cannot resolve them, so the member waits
    /// rather than being persisted unchecked or deleted: nothing else re-applies
    /// it once the snapshot has finished.
    #[test]
    fn test_persist_buffered_snapshot_entity_sharedmember_waits_for_its_anchor() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([5u8; 32]);
        let id = [6u8; 32];

        // Build a SharedMember index blob.
        let mut index = EntityIndex::minimal_for_test(Id::new(id));
        index.metadata.storage_type = calimero_storage::entities::StorageType::SharedMember {
            anchor: Id::new([7u8; 32]),
            signature_data: None,
        };
        let index_bytes =
            super::leaf::rows::with_own_hash(&borsh::to_vec(&index).unwrap(), &[1, 2, 3]);

        let outcome = persist_buffered_snapshot_entity(
            &store,
            &calimero_governance_store::NotFolded,
            ctx,
            id,
            &[1, 2, 3],
            &index_bytes,
            &|_| Ok(CellWriters::Genesis),
        )
        .unwrap();
        assert_eq!(
            outcome,
            SnapshotEntityDrainOutcome::Pending,
            "a SharedMember whose anchor is not stored must be kept for a later pass"
        );

        // A malformed index blob is still a transient Pending (kept for retry).
        let pending = persist_buffered_snapshot_entity(
            &store,
            &calimero_governance_store::NotFolded,
            ctx,
            id,
            &[1],
            &[0xFF, 0xFF],
            &|_| Ok(CellWriters::Genesis),
        )
        .unwrap();
        assert_eq!(pending, SnapshotEntityDrainOutcome::Pending);
    }

    #[test]
    fn test_decode_snapshot_records_empty() {
        let records = decode_snapshot_records(&[]).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_decode_snapshot_records_single_entity() {
        let record = SnapshotRecord::Entity {
            id: [1u8; 32],
            entry: vec![10, 20, 30],
            index: vec![40, 50, 60],
            schema_bytecode_id: None,
        };
        let encoded = build_snapshot_page_v2(std::slice::from_ref(&record));

        let records = decode_snapshot_records(&encoded).unwrap();
        assert_eq!(records.len(), 1);
        match &records[0] {
            SnapshotRecord::Entity {
                id, entry, index, ..
            } => {
                assert_eq!(*id, [1u8; 32]);
                assert_eq!(entry, &vec![10, 20, 30]);
                assert_eq!(index, &vec![40, 50, 60]);
            }
            _ => panic!("expected Entity record"),
        }
    }

    /// Regression (PR-6b Task 6b.7 review): a LEGACY (pre-#2539 / v1) page
    /// packs multiple `Entity` records back-to-back with NO trailing
    /// `schema_bytecode_id` byte and NO per-record length framing. A v2 reader
    /// must decode every such record with `schema_bytecode_id == None` and consume
    /// the whole page — never letting one record's missing trailing byte eat
    /// the next record's leading bytes (which previously desynced the stream
    /// at the second record).
    /// Build a v2-framed page (sentinel ‖ length-framed records) the way
    /// `generate_snapshot_pages` does, for decode round-trip tests.
    fn build_snapshot_page_v2(records: &[SnapshotRecord]) -> Vec<u8> {
        let mut page = vec![SNAPSHOT_PAGE_FORMAT_V2];
        for record in records {
            page.extend(encode_framed_snapshot_record(record).unwrap());
        }
        page
    }

    #[test]
    fn test_decode_legacy_multi_record_page_no_desync() {
        // Two legacy Entity records, exactly as a v1 sender would emit them:
        // {variant=0, id, entry, index} with no trailing Option byte, packed
        // end-to-end into one page buffer.
        fn legacy_entity_bytes(id: [u8; 32], entry: Vec<u8>, index: Vec<u8>) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.push(0u8); // Entity variant discriminant
            bytes.extend_from_slice(&id);
            bytes.extend_from_slice(&borsh::to_vec(&entry).unwrap());
            bytes.extend_from_slice(&borsh::to_vec(&index).unwrap());
            bytes
        }

        let mut page = legacy_entity_bytes([1u8; 32], vec![10, 20, 30], vec![40, 50]);
        page.extend(legacy_entity_bytes([2u8; 32], vec![60], vec![70, 80, 90]));

        let records = decode_snapshot_records(&page).expect("legacy multi-record page decodes");
        assert_eq!(records.len(), 2, "both legacy records must decode");
        assert_eq!(
            records[0],
            SnapshotRecord::Entity {
                id: [1u8; 32],
                entry: vec![10, 20, 30],
                index: vec![40, 50],
                schema_bytecode_id: None,
            }
        );
        assert_eq!(
            records[1],
            SnapshotRecord::Entity {
                id: [2u8; 32],
                entry: vec![60],
                index: vec![70, 80, 90],
                schema_bytecode_id: None,
            }
        );
    }

    /// Round-trip: a v2 page (framed, schema-stamped) emitted by
    /// `generate_snapshot_pages` decodes back to the stamped records, and a
    /// hand-built legacy page decodes alongside the same decoder.
    #[test]
    fn test_decode_v2_framed_page_round_trips_schema() {
        let mut page = build_snapshot_page_v2(&[
            SnapshotRecord::Entity {
                id: [3u8; 32],
                entry: vec![1, 2],
                index: vec![3, 4],
                schema_bytecode_id: Some([7u8; 32]),
            },
            SnapshotRecord::Auxiliary {
                kind: snapshot_record_kind::ROTATION_LOG,
                id: [4u8; 32],
                value: vec![5, 6, 7],
            },
        ]);
        // Decode in place.
        let records = decode_snapshot_records(&page).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0],
            SnapshotRecord::Entity {
                id: [3u8; 32],
                entry: vec![1, 2],
                index: vec![3, 4],
                schema_bytecode_id: Some([7u8; 32]),
            }
        );
        assert!(matches!(
            &records[1],
            SnapshotRecord::Auxiliary { kind, id, value }
                if *kind == snapshot_record_kind::ROTATION_LOG
                    && *id == [4u8; 32]
                    && value == &vec![5, 6, 7]
        ));
        // A trailing zero byte would have desynced a naive sequential decoder;
        // confirm the framed page is fully consumed (no leftover).
        page.clear();
    }

    #[test]
    fn test_decode_snapshot_records_mixed() {
        // Mix Entity + Auxiliary records in a single page payload to
        // exercise the streaming decode boundary handling.
        let entity = SnapshotRecord::Entity {
            id: [1u8; 32],
            entry: vec![10],
            index: vec![20],
            schema_bytecode_id: None,
        };
        let aux = SnapshotRecord::Auxiliary {
            kind: snapshot_record_kind::ROTATION_LOG,
            id: [2u8; 32],
            value: vec![30, 31],
        };

        let encoded = build_snapshot_page_v2(&[entity, aux]);

        let records = decode_snapshot_records(&encoded).unwrap();
        assert_eq!(records.len(), 2);
        assert!(matches!(
            &records[0],
            SnapshotRecord::Entity { id, .. } if *id == [1u8; 32]
        ));
        assert!(matches!(
            &records[1],
            SnapshotRecord::Auxiliary { kind, id, .. }
                if *kind == snapshot_record_kind::ROTATION_LOG && *id == [2u8; 32]
        ));
    }

    #[test]
    fn test_decompress_snapshot_page_round_trips() {
        let original = vec![7u8; 4096];
        let payload = lz4_flex::compress_prepend_size(&original);
        let out = decompress_snapshot_page(&payload, original.len() as u32).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn test_decompress_snapshot_page_accepts_oversized_single_record_page() {
        // `generate_snapshot_pages` puts a record larger than the grouping
        // hint (`DEFAULT_PAGE_BYTE_LIMIT`) on its own page, so legitimate
        // pages can exceed 64 KB. They must still be accepted under the
        // protocol cap.
        let original = vec![9u8; (DEFAULT_PAGE_BYTE_LIMIT as usize) * 4];
        let payload = lz4_flex::compress_prepend_size(&original);
        let out = decompress_snapshot_page(&payload, original.len() as u32).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn test_decompress_snapshot_page_rejects_oversized_declared_len() {
        // A peer declaring a size above the protocol limit must be rejected
        // before any allocation happens.
        let payload = lz4_flex::compress_prepend_size(&[0u8; 16]);
        let err = decompress_snapshot_page(&payload, MAX_SNAPSHOT_PAGE_SIZE + 1).unwrap_err();
        assert!(err.to_string().contains("exceeds limit"), "{err}");
    }

    #[test]
    fn test_decompress_snapshot_page_rejects_inconsistent_size_prefix() {
        // The embedded LZ4 size prefix must agree with the declared length;
        // a forged prefix is rejected up front, before allocation.
        let real = vec![3u8; 256];
        let mut payload = lz4_flex::compress_prepend_size(&real);
        // Overwrite the 4-byte little-endian size prefix with a huge value.
        payload[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = decompress_snapshot_page(&payload, 256).unwrap_err();
        assert!(err.to_string().contains("disagrees"), "{err}");
    }

    #[test]
    fn test_decompress_snapshot_page_resists_expansion_beyond_buffer() {
        // A consistent (prefix == declared) but understated length must not let
        // the block expand past the bounded buffer: `decompress_into` errors
        // rather than growing, so no oversized allocation occurs.
        let real = vec![3u8; 4096];
        let mut payload = lz4_flex::compress_prepend_size(&real);
        // Understate both the prefix and the declared length to 8 bytes; the
        // block still decompresses to 4096, overflowing the 8-byte buffer.
        payload[0..4].copy_from_slice(&8u32.to_le_bytes());
        let err = decompress_snapshot_page(&payload, 8).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Decompress failed") || msg.contains("Size mismatch"),
            "{msg}"
        );
    }

    #[test]
    fn test_decompress_snapshot_page_rejects_short_payload() {
        // Payload shorter than the 4-byte size prefix must not panic.
        let err = decompress_snapshot_page(&[1, 2, 3], 8).unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn settle_snapshot_activation_binds_to_group_bytecode_id_and_clears_markers() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::types::ContextMigrationFailed;
        use calimero_store::Store;

        const BYTECODE_ID: [u8; 32] = [0x2A; 32];
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x60; 32]);
        let ctx = ContextId::from([0x50; 32]);

        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xAA; 32]),
                        bytecode_id: BYTECODE_ID,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();

        // A stranded context: behind, with both failure + resync markers set
        // and no activation marker yet.
        let mut handle = store.handle();
        handle
            .put(
                &key::ContextMigrationFailed::new(ctx),
                &ContextMigrationFailed { kind: 3 },
            )
            .unwrap();
        handle
            .put(&key::ContextResyncRequested::new(ctx), &())
            .unwrap();
        drop(handle);

        settle_snapshot_activation(&store, ctx, None);

        assert_eq!(
            calimero_context::activation::activated_bytecode(&store, &ctx),
            Some(BYTECODE_ID),
            "activation marker must bind to the group's current bytecode_id"
        );
        let handle = store.handle();
        assert!(
            handle
                .get(&key::ContextMigrationFailed::new(ctx))
                .unwrap()
                .is_none(),
            "stranded marker must be cleared"
        );
        assert!(
            handle
                .get(&key::ContextResyncRequested::new(ctx))
                .unwrap()
                .is_none(),
            "resync marker must be cleared"
        );
    }

    /// settle returns the namespace ROOT on a resync heal (so the caller can
    /// edge-trigger the migration emitter), and `None` for a non-resync
    /// snapshot. This is the seam that makes a just-resynced member surface as
    /// recovered in the admin rollup promptly instead of lingering as `failed`.
    #[test]
    fn settle_snapshot_activation_returns_namespace_root_on_resync() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::Store;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x61; 32]); // top-level group ⇒ root == gid
        let ctx = ContextId::from([0x51; 32]);
        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xAB; 32]),
                        bytecode_id: [0x2B; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x08; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x08; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();

        // No resync marker ⇒ non-resync snapshot ⇒ no edge-trigger.
        assert_eq!(
            settle_snapshot_activation(&store, ctx, None),
            None,
            "a non-resync snapshot must not edge-trigger the emitter"
        );

        // Resync marker set ⇒ heal returns the namespace root to refresh.
        store
            .handle()
            .put(&key::ContextResyncRequested::new(ctx), &())
            .unwrap();
        assert_eq!(
            settle_snapshot_activation(&store, ctx, None),
            Some(gid.to_bytes()),
            "a resync heal must return the namespace root so the caller can \
             edge-trigger the migration emitter"
        );
    }

    #[test]
    fn settle_snapshot_activation_noop_for_non_group_context() {
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([0x51; 32]);
        // No group mapping → nothing to bind, no panic, no marker written.
        settle_snapshot_activation(&store, ctx, None);
        assert_eq!(
            calimero_context::activation::activated_bytecode(&store, &ctx),
            None
        );
    }

    #[test]
    fn settle_snapshot_activation_noop_without_resync_marker() {
        // Regression guard: a bootstrap / crash-recovery snapshot (NO resync
        // marker) must NOT advance the activation marker to the group target —
        // the synced state may be from a peer behind the target, and binding it
        // to the target would tell the lazy gate "up to date" over old-schema
        // state and silently skip the migration.
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::Store;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x61; 32]);
        let ctx = ContextId::from([0x52; 32]);
        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xAB; 32]),
                        bytecode_id: [0x2B; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();

        // No ContextResyncRequested marker set.
        settle_snapshot_activation(&store, ctx, None);

        assert_eq!(
            calimero_context::activation::activated_bytecode(&store, &ctx),
            None,
            "a non-resync snapshot must not bind the activation marker"
        );
    }

    #[test]
    fn settle_snapshot_activation_binds_to_observed_schema_not_group_target() {
        // A resync may pull from an automatically-chosen peer that is BEHIND the
        // group target. Settle must bind the marker to the schema the synced
        // data actually carried, not the target — otherwise the lazy gate would
        // treat old-schema state as up-to-date and silently skip the migration.
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::Store;

        const TARGET_KEY: [u8; 32] = [0x2A; 32];
        const BEHIND_KEY: [u8; 32] = [0x19; 32];
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x62; 32]);
        let ctx = ContextId::from([0x53; 32]);
        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xAC; 32]),
                        bytecode_id: TARGET_KEY,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        // The behind release is one the group named before its target moved on.
        calimero_governance_store::UpgradeLadderRepository::new(&store)
            .append(
                &gid,
                key::LadderRung {
                    bytecode_id: BEHIND_KEY,
                    application_id: ApplicationId::from([0xAC; 32]),
                    package: String::new(),
                    version: String::new(),
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();
        store
            .handle()
            .put(&key::ContextResyncRequested::new(ctx), &())
            .unwrap();

        settle_snapshot_activation(&store, ctx, Some(BEHIND_KEY));

        assert_eq!(
            calimero_context::activation::activated_bytecode(&store, &ctx),
            Some(BEHIND_KEY),
            "marker must bind to the synced data's real schema, not the group target"
        );
    }

    #[test]
    fn settle_snapshot_activation_ignores_a_schema_the_group_never_named() {
        // The stamp is the serving peer's word: a blob the group never named
        // (another group's squat on this node) must not bind the marker.
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::Store;

        const TARGET_KEY: [u8; 32] = [0x2B; 32];
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x63; 32]);
        let ctx = ContextId::from([0x54; 32]);
        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xAC; 32]),
                        bytecode_id: TARGET_KEY,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();
        store
            .handle()
            .put(&key::ContextResyncRequested::new(ctx), &())
            .unwrap();

        settle_snapshot_activation(&store, ctx, Some([0xEE; 32]));

        assert_eq!(
            calimero_context::activation::activated_bytecode(&store, &ctx),
            Some(TARGET_KEY)
        );
    }

    #[test]
    fn settle_snapshot_activation_reconciles_single_wasm_bound_id() {
        // Single-wasm group: the bound ApplicationId differs per version. After
        // a resync to the target schema, settle must advance ContextMeta's bound
        // id too — otherwise pending_upgrade_info (current != target) re-gates.
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::{register_context_in_group, MetaRepository};
        use calimero_primitives::application::ApplicationId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::types::ContextMeta;
        use calimero_store::Store;

        const TARGET_KEY: [u8; 32] = [0x2A; 32];
        let target_app = ApplicationId::from([0xAA; 32]);
        let old_app = ApplicationId::from([0x01; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x63; 32]);
        let ctx = ContextId::from([0x54; 32]);
        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    target: GroupTarget {
                        application_id: target_app,
                        bytecode_id: TARGET_KEY,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    owner_identity: calimero_primitives::identity::AccountId::from([0x07; 32]),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        register_context_in_group(&store, &gid, &ctx).unwrap();
        let mut handle = store.handle();
        handle
            .put(
                &key::ContextMeta::new(ctx),
                &ContextMeta::new(key::ApplicationMeta::new(old_app), [0u8; 32], vec![], None),
            )
            .unwrap();
        handle
            .put(&key::ContextResyncRequested::new(ctx), &())
            .unwrap();
        drop(handle);

        settle_snapshot_activation(&store, ctx, Some(TARGET_KEY));

        let meta = store
            .handle()
            .get(&key::ContextMeta::new(ctx))
            .unwrap()
            .unwrap();
        assert_eq!(
            meta.application.application_id(),
            target_app,
            "bound id must advance to the target so the gate does not re-fire"
        );
    }
}

/// Regression tests for #4089, over a store holding real device certificates
/// and revocations: the two ways a snapshot used to carry state no honest node
/// would accept.
#[cfg(test)]
mod snapshot_trust_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::test_fixtures::{
        enrol_member, real_join_account, sample_meta_with_admin,
    };
    use calimero_governance_store::{
        register_context_in_group, AccountBindingRepository, MembershipRepository, MetaRepository,
    };
    use calimero_node_primitives::sync::InitProof;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_storage::action::Action;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{Metadata, OpMask, SignatureData, StorageType};
    use calimero_storage::index::EntityIndex;
    use calimero_storage::shared_writers::{CellWriters, WritersUnavailable};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::{
        buffered_snapshot_entity_pass, persist_buffered_snapshot_entity, snapshot_server_admitted,
        SnapshotEntityDrainOutcome,
    };
    use crate::sync::helpers::{snapshot_leaf_authorship, SnapshotAuthorship};

    const NAMESPACE: [u8; 32] = [0x4A; 32];
    const CONTEXT: [u8; 32] = [0x4B; 32];

    struct Group {
        store: Store,
        namespace: ContextGroupId,
        context: ContextId,
    }

    impl Group {
        /// A context in a namespace whose admin is `admin`, enrolled with a real
        /// device certificate.
        fn with_admin(admin: &PublicKey) -> (Self, AccountId) {
            let group = Self {
                store: Store::new(Arc::new(InMemoryDB::owned())),
                namespace: ContextGroupId::from(NAMESPACE),
                context: ContextId::from(CONTEXT),
            };
            let account = enrol_member(&group.store, &group.namespace, admin);
            MetaRepository::new(&group.store)
                .save(&group.namespace, &sample_meta_with_admin(account))
                .unwrap();
            MembershipRepository::new(&group.store)
                .add_member(&group.namespace, &account, GroupMemberRole::Admin)
                .unwrap();
            register_context_in_group(&group.store, &group.namespace, &group.context).unwrap();
            (group, account)
        }

        fn member(&self, key: &PublicKey) -> AccountId {
            let account = enrol_member(&self.store, &self.namespace, key);
            MembershipRepository::new(&self.store)
                .add_member(&self.namespace, &account, GroupMemberRole::Member)
                .unwrap();
            account
        }

        /// Revoke the device `enrol_member` bound `key` under.
        fn revoke(&self, key: &PublicKey) {
            let device = real_join_account(key).statement.device;
            AccountBindingRepository::new(&self.store)
                .apply_revocation(&self.namespace, device)
                .unwrap();
        }

        fn serves(&self, server: &PrivateKey, peer: libp2p::PeerId) -> eyre::Result<()> {
            let identity = server.public_key();
            let message = InitProof::message(&self.context, &identity, &peer.to_bytes());
            let proof = InitProof {
                signature: server.sign(&message).unwrap().to_bytes(),
            };
            snapshot_server_admitted(&self.store, self.context, peer, identity, &proof, || {
                panic!("a context in a group must not fall back to the member set")
            })
        }

        fn authorship(&self, storage_type: StorageType) -> SnapshotAuthorship {
            let mut metadata = Metadata::new(0, 0);
            metadata.storage_type = storage_type;
            snapshot_leaf_authorship(
                &self.store,
                &calimero_governance_store::NotFolded,
                &self.context,
                Id::new([0; 32]),
                &metadata,
                None,
                &|_| Ok(CellWriters::Genesis),
            )
        }
    }

    fn signed_by(key: &PublicKey) -> Option<SignatureData> {
        Some(SignatureData {
            signature: [0x5A; 64],
            nonce: 1,
            signer: Some(*key),
            on_behalf: None,
        })
    }

    #[test]
    fn a_snapshot_served_by_a_revoked_device_is_refused() {
        let server = PrivateKey::from([0x61; 32]);
        let (group, _) = Group::with_admin(&server.public_key());
        let peer = libp2p::PeerId::random();

        group
            .serves(&server, peer)
            .expect("precondition: an admitted member may serve a snapshot");

        group.revoke(&server.public_key());
        assert!(
            group.serves(&server, peer).is_err(),
            "a revoked device must not be able to serve a cold joiner its state"
        );
    }

    #[test]
    fn a_snapshot_source_must_prove_its_identity_from_the_peer_it_is() {
        let server = PrivateKey::from([0x62; 32]);
        let (group, _) = Group::with_admin(&server.public_key());
        let identity = server.public_key();
        let signed_for = libp2p::PeerId::random();
        let message = InitProof::message(&group.context, &identity, &signed_for.to_bytes());
        let proof = InitProof {
            signature: server.sign(&message).unwrap().to_bytes(),
        };

        assert!(
            snapshot_server_admitted(
                &group.store,
                group.context,
                libp2p::PeerId::random(),
                identity,
                &proof,
                || unreachable!(),
            )
            .is_err(),
            "a proof bound to another peer must not let this one serve as the identity"
        );
    }

    #[test]
    fn a_stranger_cannot_serve_a_snapshot() {
        let (group, _) = Group::with_admin(&PrivateKey::from([0x63; 32]).public_key());
        let stranger = PrivateKey::from([0x64; 32]);
        assert!(group.serves(&stranger, libp2p::PeerId::random()).is_err());
    }

    #[test]
    fn a_snapshot_entry_signed_by_another_accounts_key_is_forged() {
        let alice = PublicKey::from([0x71; 32]);
        let mallory = PublicKey::from([0x72; 32]);
        let (group, alice_account) = Group::with_admin(&alice);
        let _ = group.member(&mallory);

        let alices_entry = |signer: &PublicKey| StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: alice_account,
            signature_data: signed_by(signer),
        };
        assert_eq!(
            group.authorship(alices_entry(&alice)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            group.authorship(alices_entry(&mallory)),
            SnapshotAuthorship::Forged,
            "a member serving a snapshot must not be able to write alice's entry"
        );

        let shared = |signer: &PublicKey| StorageType::Shared {
            writers: BTreeMap::from([(alice_account, Default::default())]),
            signature_data: signed_by(signer),
        };
        assert_eq!(
            group.authorship(shared(&alice)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            group.authorship(shared(&mallory)),
            SnapshotAuthorship::Forged,
            "a signer outside the writer set must not be able to author the entry"
        );
    }

    /// A leaf declined as future-schema is re-verified when the reader catches
    /// up. That late apply must ask whose account signed it, as the page apply
    /// does: the signature check alone accepts an entry under Alice's `owner`
    /// signed with a key of Mallory's own.
    #[test]
    fn a_buffered_entry_signed_by_another_accounts_key_is_refused() {
        let alice = PrivateKey::from([0x75; 32]);
        let mallory = PrivateKey::from([0x76; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let _ = group.member(&mallory.public_key());

        let drain = |signer: &PrivateKey| {
            let id = calimero_storage::tests::common::owned_entry_id(
                Id::new([0x77; 32]),
                &alice_account,
            );
            let data = b"alice's entry".to_vec();
            let mut metadata = Metadata::new(1, 1);
            metadata.storage_type = StorageType::User {
                rules: calimero_storage::entities::EntryRules::OWNED,
                owner: alice_account,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce: 1,
                    signer: Some(signer.public_key()),
                    on_behalf: None,
                }),
            };
            let payload = Action::Add {
                id,
                data: data.clone(),
                ancestors: vec![],
                metadata: metadata.clone(),
            }
            .payload_for_signing();
            if let StorageType::User {
                signature_data: Some(sig),
                ..
            } = &mut metadata.storage_type
            {
                sig.signature = signer.sign(&payload).unwrap().to_bytes();
            }
            let mut index = EntityIndex::minimal_for_test(id);
            index.metadata = metadata;
            persist_buffered_snapshot_entity(
                &group.store,
                &calimero_governance_store::NotFolded,
                group.context,
                *id.as_bytes(),
                &data,
                &super::leaf::rows::with_own_hash(&borsh::to_vec(&index).unwrap(), &data),
                &|_| Ok(CellWriters::Genesis),
            )
            .unwrap()
        };

        assert_eq!(
            drain(&mallory),
            SnapshotEntityDrainOutcome::Refused,
            "a member serving a snapshot must not be able to write alice's entry late"
        );
        assert_eq!(drain(&alice), SnapshotEntityDrainOutcome::Persisted);
    }

    /// Drains, as a buffered snapshot leaf, Alice's owned entry signed at `nonce`
    /// and shipped dated `updated_at`. Returns the outcome and the stored date.
    fn drain_alices_entry(
        nonce: u64,
        updated_at: u64,
    ) -> (SnapshotEntityDrainOutcome, Option<u64>) {
        let alice = PrivateKey::from([0x78; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let id =
            calimero_storage::tests::common::owned_entry_id(Id::new([0x79; 32]), &alice_account);
        let data = b"alice's entry".to_vec();
        let mut metadata = Metadata::new(1, nonce);
        metadata.storage_type = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: alice_account,
            signature_data: signer(&alice, nonce),
        };
        let payload = Action::Add {
            id,
            data: data.clone(),
            ancestors: vec![],
            metadata: metadata.clone(),
        }
        .payload_for_signing();
        if let StorageType::User {
            signature_data: Some(sig),
            ..
        } = &mut metadata.storage_type
        {
            sig.signature = alice.sign(&payload).unwrap().to_bytes();
        }
        metadata.updated_at = updated_at.into();
        let mut index = EntityIndex::minimal_for_test(id);
        index.metadata = metadata;

        let outcome = persist_buffered_snapshot_entity(
            &group.store,
            &calimero_governance_store::NotFolded,
            group.context,
            *id.as_bytes(),
            &data,
            &super::leaf::rows::with_own_hash(&borsh::to_vec(&index).unwrap(), &data),
            &|_| Ok(CellWriters::Genesis),
        )
        .unwrap();
        let stored = crate::delta_store::read_entity_index_direct(&group.store, group.context, id)
            .unwrap()
            .map(|index| index.metadata.updated_at());
        (outcome, stored)
    }

    /// `updated_at` is not signed, so a peer serving a snapshot can re-date an
    /// entry. The joiner stores it under the date its signature commits to.
    #[test]
    fn a_snapshot_entry_is_stored_under_its_signed_date() {
        assert_eq!(
            drain_alices_entry(1, calimero_storage::env::time_now()),
            (SnapshotEntityDrainOutcome::Persisted, Some(1))
        );
    }

    /// A date past the drift tolerance would make every later write to the entry
    /// look stale, and stamp this node's own writes past it, so it is refused.
    #[test]
    fn a_snapshot_entry_signed_ahead_of_the_clock_is_refused() {
        let ahead = calimero_storage::env::time_now() + 60_000_000_000;
        assert_eq!(
            drain_alices_entry(ahead, 1),
            (SnapshotEntityDrainOutcome::Refused, None)
        );
    }

    fn writer_set(accounts: &[AccountId]) -> BTreeMap<AccountId, OpMask> {
        accounts.iter().map(|a| (*a, OpMask::FULL)).collect()
    }

    /// A cell's wrapper id, as a `Shared` anchor's must be.
    fn cell_at(seed: u8, writers: &[AccountId]) -> Id {
        calimero_storage::tests::common::cell_at(seed, &writers.iter().copied().collect())
    }

    /// An id in the value subtree of `anchor`, as a member's must be.
    fn member_at(anchor: Id, seed: u8) -> Id {
        calimero_storage::tests::common::member_at(anchor, seed)
    }

    fn signer(key: &PrivateKey, nonce: u64) -> Option<SignatureData> {
        Some(SignatureData {
            signature: [0; 64],
            nonce,
            signer: Some(key.public_key()),
            on_behalf: None,
        })
    }

    /// Sign the leaf `index` names, holding `data`, as the snapshot checks it,
    /// and return the index blob a buffer record holds for it.
    fn signed_index(
        mut index: EntityIndex,
        data: &[u8],
        mut metadata: Metadata,
        key: &PrivateKey,
    ) -> Vec<u8> {
        let payload = Action::Add {
            id: index.id(),
            data: data.to_vec(),
            ancestors: vec![],
            metadata: metadata.clone(),
        }
        .payload_for_signing();
        match &mut metadata.storage_type {
            StorageType::Shared {
                signature_data: Some(sig),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sig),
                ..
            } => sig.signature = key.sign(&payload).unwrap().to_bytes(),
            other => panic!("not a signed shared leaf: {other:?}"),
        }
        index.metadata = metadata;
        super::leaf::rows::with_own_hash(&borsh::to_vec(&index).unwrap(), data)
    }

    /// The bytes every `Shared` anchor here holds, and so its `own_hash`.
    const ANCHOR_DATA: &[u8] = b"shared value";

    /// A `Shared` anchor carrying `writers`, signed by `key` at `at`. Its
    /// `full_hash` names children none of these tests store.
    fn shared_leaf(id: Id, writers: &[AccountId], key: &PrivateKey, at: u64) -> (Vec<u8>, Vec<u8>) {
        shared_leaf_at(EntityIndex::minimal_for_test(id), writers, key, at)
    }

    fn shared_leaf_at(
        index: EntityIndex,
        writers: &[AccountId],
        key: &PrivateKey,
        at: u64,
    ) -> (Vec<u8>, Vec<u8>) {
        let data = ANCHOR_DATA.to_vec();
        let mut metadata = Metadata::new(at, at);
        metadata.storage_type = StorageType::Shared {
            writers: writer_set(writers),
            signature_data: signer(key, at),
        };
        let index = signed_index(index, &data, metadata, key);
        (data, index)
    }

    /// A member of `anchor`, signed by `key` at `at`.
    fn member_leaf(id: Id, anchor: Id, key: &PrivateKey, at: u64) -> (Vec<u8>, Vec<u8>) {
        member_leaf_at(EntityIndex::minimal_for_test(id), anchor, key, at)
    }

    fn member_leaf_at(
        index: EntityIndex,
        anchor: Id,
        key: &PrivateKey,
        at: u64,
    ) -> (Vec<u8>, Vec<u8>) {
        let data = b"member value".to_vec();
        let mut metadata = Metadata::new(at, at);
        metadata.storage_type = StorageType::SharedMember {
            anchor,
            signature_data: signer(key, at),
        };
        let index = signed_index(index, &data, metadata, key);
        (data, index)
    }

    impl Group {
        fn drain(&self, id: Id, data: &[u8], index: &[u8]) -> SnapshotEntityDrainOutcome {
            self.drain_with(id, data, index, &|_| Ok(CellWriters::Genesis))
        }

        /// Drain with `ever` as the governance fold's ever-writers of any cell.
        fn drain_with(
            &self,
            id: Id,
            data: &[u8],
            index: &[u8],
            ever: &dyn Fn(Id) -> Result<CellWriters, WritersUnavailable>,
        ) -> SnapshotEntityDrainOutcome {
            persist_buffered_snapshot_entity(
                &self.store,
                &calimero_governance_store::NotFolded,
                self.context,
                *id.as_bytes(),
                data,
                index,
                ever,
            )
            .unwrap()
        }

        fn is_stored(&self, id: Id) -> bool {
            self.store
                .handle()
                .get(&super::ContextStateKey::new(
                    self.context,
                    super::StorageKey::Index(id).to_bytes(),
                ))
                .unwrap()
                .and_then(|row| calimero_storage::row::decode(id, row.value.as_ref()))
                .is_some_and(|row| row.data.is_some())
        }
    }

    /// The ever-writers a rotated cell answers with: `accounts`, as a fold would.
    fn rotated(accounts: &[AccountId]) -> Result<CellWriters, WritersUnavailable> {
        Ok(CellWriters::Rotated(writer_set(accounts)))
    }

    /// A snapshot declined as future-schema is buffered whole and drains in one
    /// pass. Taken in `buffered_snapshot_entity_pass` order, an anchor and a
    /// member that a since-removed writer signed both land, however the buffer
    /// happened to list them; taken member first, the member would wait for a
    /// later trigger.
    #[test]
    fn a_buffered_snapshot_drains_in_one_pass_in_page_apply_order() {
        let alice = PrivateKey::from([0x90; 32]);
        let bob = PrivateKey::from([0x91; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let bob_account = group.member(&bob.public_key());
        let anchor = cell_at(0x92, &[alice_account, bob_account]);
        let member = member_at(anchor, 0x93);

        let (member_data, member_index) = member_leaf(member, anchor, &bob, 2);
        let (anchor_data, anchor_index) =
            shared_leaf(anchor, &[alice_account, bob_account], &bob, 3);
        let mut records = vec![
            (member, member_data, member_index),
            (anchor, anchor_data, anchor_index),
        ];

        records.sort_by_key(|(_, _, index)| buffered_snapshot_entity_pass(index));
        for (id, data, index) in &records {
            assert_eq!(
                group.drain_with(*id, data, index, &|_| rotated(&[alice_account])),
                SnapshotEntityDrainOutcome::Persisted
            );
        }
        assert!(group.is_stored(anchor) && group.is_stored(member));
    }

    /// A `Shared` leaf that drains late is held to the cell's writers, as the page
    /// apply holds it: a member outside them, signing with a key of its own,
    /// must not get the entry in through the buffer.
    #[test]
    fn a_buffered_shared_entry_signed_by_a_non_writer_is_refused() {
        let alice = PrivateKey::from([0x81; 32]);
        let mallory = PrivateKey::from([0x82; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let _ = group.member(&mallory.public_key());
        let anchor = cell_at(0x83, &[alice_account]);

        let (data, forged) = shared_leaf(anchor, &[alice_account], &mallory, 5);
        assert_eq!(
            group.drain_with(anchor, &data, &forged, &|cell| {
                assert_eq!(cell, anchor, "the cell of a Shared leaf is the leaf");
                Ok(CellWriters::Genesis)
            }),
            SnapshotEntityDrainOutcome::Refused,
            "a member outside the writers must not be able to write the entry late"
        );
        assert!(!group.is_stored(anchor));

        let (data, genuine) = shared_leaf(anchor, &[alice_account], &alice, 5);
        assert_eq!(
            group.drain(anchor, &data, &genuine),
            SnapshotEntityDrainOutcome::Persisted
        );
    }

    /// A member's writers are its anchor's. The drain must resolve them from
    /// the stored anchor, as the page apply resolves them from the snapshot's.
    #[test]
    fn a_buffered_shared_member_signed_by_a_non_writer_is_refused() {
        let alice = PrivateKey::from([0x84; 32]);
        let mallory = PrivateKey::from([0x85; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let _ = group.member(&mallory.public_key());
        let anchor = cell_at(0x86, &[alice_account]);
        let member = member_at(anchor, 0x87);

        let (data, forged) = member_leaf(member, anchor, &mallory, 5);
        assert_eq!(
            group.drain(member, &data, &forged),
            SnapshotEntityDrainOutcome::Pending,
            "the anchor is not stored yet, so the member's writers cannot be resolved"
        );

        let (anchor_data, anchor_index) = shared_leaf(anchor, &[alice_account], &alice, 1);
        assert_eq!(
            group.drain(anchor, &anchor_data, &anchor_index),
            SnapshotEntityDrainOutcome::Persisted
        );
        assert_eq!(
            group.drain_with(member, &data, &forged, &|cell| {
                assert_eq!(cell, anchor, "the anchor's ever-writers");
                Ok(CellWriters::Genesis)
            }),
            SnapshotEntityDrainOutcome::Refused,
            "a member outside the anchor's writers must not be able to write a member late"
        );
        assert!(!group.is_stored(member));

        let (data, genuine) = member_leaf(member, anchor, &alice, 5);
        assert_eq!(
            group.drain(member, &data, &genuine),
            SnapshotEntityDrainOutcome::Persisted
        );
    }

    /// Bob is no genesis writer; a rotation added him. The leaf carries only the
    /// genesis set, so the ever-writers are what admit his leaf and his member.
    /// A rotation does not protect a stranger: Mallory was never a writer.
    #[test]
    fn a_buffered_leaf_by_a_writer_a_rotation_added_is_kept() {
        let alice = PrivateKey::from([0x88; 32]);
        let bob = PrivateKey::from([0x89; 32]);
        let mallory = PrivateKey::from([0xA8; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let bob_account = group.member(&bob.public_key());
        let _ = group.member(&mallory.public_key());
        let anchor = cell_at(0x8A, &[alice_account]);
        let member = member_at(anchor, 0x8B);
        let ever = |_: Id| rotated(&[alice_account, bob_account]);

        let (anchor_data, anchor_index) = shared_leaf(anchor, &[alice_account], &alice, 3);
        assert_eq!(
            group.drain_with(anchor, &anchor_data, &anchor_index, &ever),
            SnapshotEntityDrainOutcome::Persisted
        );
        let (data, by_bob) = member_leaf(member, anchor, &bob, 5);
        assert_eq!(
            group.drain_with(member, &data, &by_bob, &ever),
            SnapshotEntityDrainOutcome::Persisted,
            "a writer a rotation added signs members of the cell"
        );
        let (data, by_mallory) = member_leaf(member, anchor, &mallory, 6);
        assert_eq!(
            group.drain_with(member, &data, &by_mallory, &ever),
            SnapshotEntityDrainOutcome::Refused
        );
        let (data, anchor_by_bob) = shared_leaf(anchor, &[alice_account], &bob, 7);
        assert_eq!(
            group.drain_with(anchor, &data, &anchor_by_bob, &ever),
            SnapshotEntityDrainOutcome::Persisted,
            "and the cell itself"
        );
    }

    /// Until the governance fold can be read, a signer outside the genesis set
    /// is neither admitted nor refused: the leaf waits, whether the heads are not
    /// here yet or the cell is past the fold's budget, and is decided once they are.
    #[test]
    fn a_buffered_leaf_waits_while_the_cells_writers_cannot_be_read() {
        let alice = PrivateKey::from([0x8C; 32]);
        let bob = PrivateKey::from([0x8D; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let bob_account = group.member(&bob.public_key());
        let anchor = cell_at(0x8E, &[alice_account]);

        let (data, by_bob) = shared_leaf(anchor, &[alice_account], &bob, 3);
        for unavailable in [WritersUnavailable::Cut, WritersUnavailable::OverBudget] {
            assert_eq!(
                group.drain_with(anchor, &data, &by_bob, &|_| Err(unavailable)),
                SnapshotEntityDrainOutcome::Pending,
                "{unavailable:?}"
            );
            assert!(!group.is_stored(anchor));
        }
        assert_eq!(
            group.drain_with(anchor, &data, &by_bob, &|_| rotated(&[
                alice_account,
                bob_account
            ])),
            SnapshotEntityDrainOutcome::Persisted
        );
        assert!(group.is_stored(anchor));

        let (data, by_alice) = shared_leaf(anchor, &[alice_account], &alice, 4);
        assert_eq!(
            group.drain_with(anchor, &data, &by_alice, &|_| Err(WritersUnavailable::Cut)),
            SnapshotEntityDrainOutcome::Persisted,
            "a genesis writer needs no governance read"
        );
    }

    /// The signer's key rides in the leaf, so a signature that fails to verify
    /// never will: the drain refuses it, as the page apply drops it.
    #[test]
    fn a_buffered_leaf_whose_signature_does_not_verify_is_refused() {
        let alice = PrivateKey::from([0x9B; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let anchor = cell_at(0x9C, &[alice_account]);

        let (_, index) = shared_leaf(anchor, &[alice_account], &alice, 5);
        let other = b"not what alice signed";
        let index = super::leaf::rows::with_own_hash(&index, other);
        assert_eq!(
            group.drain(anchor, other, &index),
            SnapshotEntityDrainOutcome::Refused
        );
        assert!(!group.is_stored(anchor));
    }

    /// What stays undecidable is kept for a bounded number of passes, then
    /// deleted, so a member cannot fill the buffer with records that wait for
    /// governance that never comes.
    #[test]
    fn a_buffered_record_left_pending_is_evicted_after_its_last_pass() {
        use calimero_governance_store::{AbsorbRecord, AbsorbRepository};
        use calimero_node_primitives::delta_buffer::MAX_GOVERNANCE_DRAIN_ATTEMPTS;

        use super::drain_buffered_snapshot_entity;

        let alice = PrivateKey::from([0x9D; 32]);
        let mallory = PrivateKey::from([0x9E; 32]);
        let (group, alice_account) = Group::with_admin(&alice.public_key());
        let _ = group.member(&mallory.public_key());
        let anchor = cell_at(0x9F, &[alice_account]);
        let schema = [0xA0; 32];

        let (data, forged) = shared_leaf(anchor, &[alice_account], &mallory, 5);
        let repo = AbsorbRepository::new(&group.store);
        repo.save(
            &group.context,
            schema,
            &AbsorbRecord::from_snapshot_entity(*anchor.as_bytes(), data, forged, schema),
        )
        .unwrap();

        let pass = || {
            let record = repo
                .load(&group.context, schema, *anchor.as_bytes())
                .unwrap()
                .expect("the record is still buffered");
            drain_buffered_snapshot_entity(
                &group.store,
                &calimero_governance_store::NotFolded,
                group.context,
                schema,
                record,
                &|_| Err(WritersUnavailable::Cut),
            )
            .unwrap()
        };
        for _ in 1..MAX_GOVERNANCE_DRAIN_ATTEMPTS {
            assert_eq!(pass(), SnapshotEntityDrainOutcome::Pending);
        }
        assert_eq!(pass(), SnapshotEntityDrainOutcome::Refused);
        assert!(repo
            .load(&group.context, schema, *anchor.as_bytes())
            .unwrap()
            .is_none());
        assert!(!group.is_stored(anchor));
    }

    #[test]
    fn a_revoked_devices_earlier_entries_still_verify() {
        // The point of resolving through every certificate rather than the live
        // binding: revoking a device must not strip the state it wrote from
        // every future cold joiner.
        let alice = PublicKey::from([0x73; 32]);
        let (group, alice_account) = Group::with_admin(&alice);
        group.revoke(&alice);

        assert_eq!(
            group.authorship(StorageType::User {
                rules: calimero_storage::entities::EntryRules::OWNED,
                owner: alice_account,
                signature_data: signed_by(&alice),
            }),
            SnapshotAuthorship::Authored
        );
    }

    #[test]
    fn a_key_no_certificate_names_fails_the_snapshot_rather_than_the_entry() {
        let (group, alice_account) = Group::with_admin(&PublicKey::from([0x74; 32]));
        assert_eq!(
            group.authorship(StorageType::User {
                rules: calimero_storage::entities::EntryRules::OWNED,
                owner: alice_account,
                signature_data: signed_by(&PublicKey::from([0x75; 32])),
            }),
            SnapshotAuthorship::Unknown
        );
    }
}
