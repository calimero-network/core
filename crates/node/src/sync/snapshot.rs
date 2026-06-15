//! Snapshot sync protocol for full state bootstrap.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use borsh::BorshDeserialize;
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
use calimero_storage::env::time_now;
use calimero_storage::interface::Interface;
use calimero_storage::store::{Key as StorageKey, MainStorage};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::key::{Generic as GenericKey, SCOPE_SIZE};
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use eyre::Result;
use hex;
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

/// Maximum uncompressed bytes per snapshot page (64 KB).
pub const DEFAULT_PAGE_BYTE_LIMIT: u32 = 64 * 1024;

/// Maximum pages to send in a single burst.
pub const DEFAULT_PAGE_LIMIT: u16 = 16;

/// Leading byte of a **v2** (PR-6b / #2539) snapshot page: records are
/// length-framed (`u32 LE len ‖ record_bytes`) so the receiver bounds each
/// record's decode to its own sub-slice. This makes the backward-compatible
/// trailing `SnapshotRecord::Entity.schema_app_key` (decoded EOF-tolerantly)
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

/// Whether a snapshot `Entity` whose sender stamped `schema_app_key` is readable
/// by a receiver whose loaded reader is `loaded_app_key` (PR-6b Task 6b.7).
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
    schema_app_key: Option<[u8; 32]>,
    loaded_app_key: Option<[u8; 32]>,
) -> bool {
    match (schema_app_key, loaded_app_key) {
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

        info!(
            %context_id,
            root_hash = %context.root_hash,
            heads_count = context.dag_heads.len(),
            "Sending snapshot boundary response"
        );

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SnapshotBoundaryResponse {
                boundary_timestamp: time_now(),
                boundary_root_hash: context.root_hash,
                dag_heads: context.dag_heads.clone(),
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
        // Verify boundary is still valid
        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                warn!(%context_id, "Context not found for snapshot stream");
                return self
                    .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                    .await;
            }
        };

        if context.root_hash != boundary_root_hash {
            warn!(%context_id, "Boundary mismatch - state changed during sync");
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
        let schema_app_key = calimero_context::hlc_fence::loaded_reader_app_key(
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
            schema_app_key,
        )?;

        // Post-iteration recheck: verify root hash hasn't changed during page generation.
        // This is a safety guardrail in addition to the RocksDB snapshot iterator.
        let current_context = self.context_client.get_context(&context_id)?;
        if let Some(ctx) = current_context {
            if ctx.root_hash != boundary_root_hash {
                warn!(
                    %context_id,
                    expected = %boundary_root_hash,
                    actual = %ctx.root_hash,
                    "Root hash changed during snapshot generation"
                );
                return self
                    .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                    .await;
            }
        }

        info!(%context_id, pages = pages.len(), total_entries, "Streaming snapshot");

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
        // An operator-requested resync (the recovery for a stranded
        // NoMigrationPath context) admits a full-state replacement too — it is
        // an explicit, force-gated request to adopt a peer's current state.
        let is_resync_requested = self
            .context_client
            .datastore_handle()
            .get(&calimero_store::key::ContextResyncRequested::new(
                context_id.into(),
            ))?
            .is_some();
        if !force && !is_crash_recovery && !is_resync_requested {
            // Check both state keys and context metadata to determine initialization.
            // A context is considered initialized if:
            // 1. It has state keys, OR
            // 2. It has a non-zero root_hash in metadata (can happen after deletes)
            let handle = self.context_client.datastore_handle();
            let has_state_keys = has_context_state_keys(&handle, context_id)?;

            let has_initialized_metadata = self
                .context_client
                .get_context(&context_id)?
                .map(|ctx| *ctx.root_hash != [0u8; 32])
                .unwrap_or(false);

            let is_initialized = has_state_keys || has_initialized_metadata;
            calimero_node_primitives::sync::check_snapshot_safety(is_initialized)
                .map_err(|e| eyre::eyre!("Snapshot safety check failed: {:?}", e))?;
        }

        let mut stream = self.sync_network.open_stream(peer_id).await?;
        let boundary = self
            .request_snapshot_boundary(context_id, &mut stream)
            .await?;

        info!(%context_id, root_hash = %boundary.boundary_root_hash, "Received boundary");

        let applied_records = self
            .request_and_apply_snapshot_pages(context_id, &boundary, &mut stream)
            .await?;

        // Verify snapshot integrity by computing the actual root hash from storage (I7).
        // On success we always trust the locally-computed hash because it reflects what
        // is actually persisted -- storing the peer's claimed hash when it disagrees
        // with local storage would create a silent divergence.
        // On failure (deserialization error) we fall back to the peer's claimed hash so
        // that sync can still proceed; compute_root_hash may fail if the minimal structs
        // drift from the real storage layout.
        let root_to_store = match self.context_client.compute_root_hash(&context_id) {
            Ok(computed_root) => {
                if computed_root != *boundary.boundary_root_hash {
                    warn!(
                        %context_id,
                        computed_root = %hex::encode(computed_root),
                        claimed_root = %hex::encode(*boundary.boundary_root_hash),
                        "Snapshot root hash mismatch - using computed hash from storage"
                    );
                } else {
                    info!(
                        %context_id,
                        root_hash = %hex::encode(computed_root),
                        "Snapshot root hash verified successfully"
                    );
                }
                computed_root
            }
            Err(e) => {
                warn!(
                    %context_id,
                    error = %e,
                    claimed_root = %hex::encode(*boundary.boundary_root_hash),
                    "Could not compute root hash, trusting peer's claimed hash"
                );
                *boundary.boundary_root_hash
            }
        };

        self.context_client
            .force_root_hash(&context_id, root_to_store.into())?;
        self.context_client
            .update_dag_heads(&context_id, boundary.dag_heads.clone())?;
        self.clear_sync_in_progress_marker(context_id)?;
        self.finalize_snapshot_activation(context_id);

        info!(%context_id, applied_records, "Snapshot sync completed successfully");

        Ok(SnapshotSyncResult {
            boundary_root_hash: boundary.boundary_root_hash,
            dag_heads: boundary.dag_heads,
            applied_records,
        })
    }

    /// Request snapshot boundary from a peer.
    async fn request_snapshot_boundary(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
    ) -> Result<SnapshotBoundary> {
        use calimero_node_primitives::sync::InitPayload;

        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) =
            crate::utils::choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()?
        else {
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
            } => Ok(SnapshotBoundary {
                boundary_timestamp,
                boundary_root_hash,
                dag_heads,
            }),
            MessagePayload::SnapshotError { error } => {
                eyre::bail!("Snapshot boundary request failed: {:?}", error);
            }
            _ => eyre::bail!("Unexpected payload in snapshot boundary response"),
        }
    }

    /// Request and apply snapshot pages from a peer.
    ///
    /// This method uses an atomic approach to avoid leaving the node in a
    /// partially cleared state if the stream fails:
    /// 1. Set a sync-in-progress marker for crash recovery detection
    /// 2. Receive all pages and write new keys (overwriting existing ones)
    /// 3. Track which keys we received from the snapshot
    /// 4. After completion, delete any old keys not in the new snapshot
    /// 5. Remove the sync-in-progress marker (after metadata update)
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
    async fn request_and_apply_snapshot_pages(
        &self,
        context_id: ContextId,
        boundary: &SnapshotBoundary,
        stream: &mut Stream,
    ) -> Result<usize> {
        use calimero_node_primitives::sync::InitPayload;

        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) =
            crate::utils::choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()?
        else {
            eyre::bail!("No owned identity found for context: {}", context_id);
        };

        // Set sync-in-progress marker for crash recovery detection
        self.set_sync_in_progress_marker(context_id, &boundary.boundary_root_hash)?;

        // Wall-clock start, for the snapshot-progress ETA estimate.
        let started_at = Instant::now();

        // Collect existing keys BEFORE receiving any pages
        // We'll use this to determine which keys to delete after sync completes
        let existing_keys: HashSet<[u8; 32]> = {
            let handle = self.context_client.datastore_handle();
            collect_context_state_keys(&handle, context_id)?
                .into_iter()
                .collect()
        };
        debug!(%context_id, existing_count = existing_keys.len(), "Collected existing state keys");

        // Track keys received from the snapshot (to know what to keep).
        // Includes Entry + Index keys for every `SnapshotRecord::Entity`
        // we accept after signature verification. We *also* insert the
        // entity's `RotationLog` state_key here even though snapshot
        // doesn't ship rotation logs (intentional, per the #2387
        // security trade-off — see the receiver's Auxiliary reject
        // path). Without that, any rotation history the receiver
        // built up from verified delta replay would be wiped by
        // `cleanup_stale_keys` at the end of the snapshot, since the
        // RotationLog state_key sits in `existing_keys` but never in
        // `received_keys`. Preserving it lets `writers_at(causal_point)`
        // lookups keep working on post-snapshot delta applies.
        let mut received_keys: HashSet<[u8; 32]> = HashSet::new();
        let mut total_applied = 0;
        let mut resume_cursor: Option<Vec<u8>> = None;

        // PR-6b Task 6b.7: the schema this node can read *right now* (its loaded
        // reader). A snapshot `Entity` whose sender stamped a newer
        // `schema_app_key` is declined+buffered rather than `handle.put`-stored.
        // `None` (non-group context / unresolvable meta) ⇒ no gate — apply as
        // today (parity with the leaf path's `handle_entity_push`).
        let loaded_app_key = calimero_context::hlc_fence::loaded_reader_app_key(
            self.context_client.datastore(),
            &context_id,
        )
        .ok()
        .flatten();

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
            BTreeMap<calimero_primitives::identity::PublicKey, calimero_storage::entities::OpMask>,
        > = HashMap::new();
        let mut deferred_members: Vec<(Id, Vec<u8>, Vec<u8>)> = Vec::new();

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
                        // Handle empty snapshot (no entries)
                        if payload.is_empty() && uncompressed_len == 0 {
                            // Empty snapshot - delete all existing keys
                            self.cleanup_stale_keys(context_id, &existing_keys, &received_keys)?;
                            return Ok(total_applied);
                        }

                        let decompressed = decompress_snapshot_page(&payload, uncompressed_len)?;

                        let records = decode_snapshot_records(&decompressed)?;
                        let mut handle = self.context_client.datastore_handle();
                        let mut applied = 0usize;
                        let mut rejected = 0usize;
                        // Note: each `SnapshotRecord::Entity` is
                        // written here via raw `handle.put` after
                        // `verify_snapshot_entity_signature` passes —
                        // we deliberately do NOT route through
                        // `Interface::apply_action` (and therefore
                        // skip nonce-replay protection / CRDT merge
                        // on the snapshot apply path). Safety
                        // invariant: `request_snapshot_sync` rejects
                        // snapshots when the local context is
                        // already initialized (Invariant I5 — see
                        // the `check_snapshot_safety` gate at the
                        // top of `request_snapshot_sync`). The only
                        // bypass is `force = true` which is reserved
                        // for crash recovery, where the marker file
                        // confirms we were already mid-snapshot and
                        // the local state is known-incomplete.
                        // Under either gate, the receiver doesn't
                        // hold "newer state" that an older-nonce
                        // snapshot could clobber. If that invariant
                        // ever loosens, this path needs the nonce
                        // / CRDT-merge logic that
                        // `apply_leaf_with_crdt_merge` provides.
                        for record in &records {
                            match record {
                                SnapshotRecord::Entity {
                                    id,
                                    entry,
                                    index,
                                    schema_app_key,
                                } => {
                                    // PR-6b Task 6b.7: the snapshot apply path
                                    // writes verified entities via a raw
                                    // `handle.put`, bypassing the gossip
                                    // state-delta fence AND
                                    // `apply_leaf_with_crdt_merge_gated`. So the
                                    // readability check has to live here: if the
                                    // sender stamped a `schema_app_key` newer
                                    // than this node's loaded reader, DECLINE +
                                    // BUFFER the raw entity into the absorb
                                    // buffer instead of persisting unreadable
                                    // bytes. The buffered entity is re-applied
                                    // (re-verified + `handle.put`) once the
                                    // loaded reader advances to that schema.
                                    if !snapshot_entity_is_readable(*schema_app_key, loaded_app_key)
                                    {
                                        if let Err(e) = self.buffer_future_schema_snapshot_entity(
                                            context_id,
                                            *id,
                                            entry,
                                            index,
                                            schema_app_key.expect("gate only declines when Some"),
                                        ) {
                                            warn!(
                                                %context_id,
                                                id = ?id,
                                                error = ?e,
                                                "snapshot Entity record: failed to buffer \
                                                 future-schema entity into the absorb buffer"
                                            );
                                        }
                                        rejected += 1;
                                        continue;
                                    }
                                    // Per-entity signature verification
                                    // (closes the peer-trust gap from
                                    // issue #2387). Parse the index
                                    // blob to recover metadata, then
                                    // run `verify_snapshot_entity_signature`
                                    // against the data + storage-type
                                    // access-control rules. Drop the
                                    // record on verification failure;
                                    // the rest of the snapshot still
                                    // applies.
                                    let index_entity: calimero_storage::index::EntityIndex =
                                        match borsh::from_slice(index) {
                                            Ok(idx) => idx,
                                            Err(e) => {
                                                warn!(
                                                    %context_id,
                                                    id = ?id,
                                                    error = ?e,
                                                    "snapshot Entity record: index blob \
                                                     failed to deserialize as EntityIndex — \
                                                     dropping"
                                                );
                                                rejected += 1;
                                                continue;
                                            }
                                        };
                                    let id_obj = Id::new(*id);

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
                                        ));
                                        continue;
                                    }

                                    if let Err(e) =
                                        Interface::<MainStorage>::verify_snapshot_entity_signature(
                                            id_obj,
                                            entry,
                                            &index_entity.metadata,
                                        )
                                    {
                                        warn!(
                                            %context_id,
                                            id = ?id,
                                            error = ?e,
                                            storage_type = ?index_entity.metadata.storage_type,
                                            "snapshot Entity record: signature \
                                             verification failed — dropping"
                                        );
                                        rejected += 1;
                                        continue;
                                    }

                                    // Verified — persist both Entry
                                    // and Index blobs under their
                                    // hashed storage keys.
                                    let entry_state_key = StorageKey::Entry(id_obj).to_bytes();
                                    let index_state_key = StorageKey::Index(id_obj).to_bytes();
                                    let entry_key =
                                        ContextStateKey::new(context_id, entry_state_key);
                                    let index_key =
                                        ContextStateKey::new(context_id, index_state_key);
                                    let entry_slice: Slice<'_> = entry.clone().into();
                                    let index_slice: Slice<'_> = index.clone().into();
                                    handle
                                        .put(&entry_key, &ContextStateValue::from(entry_slice))?;
                                    handle
                                        .put(&index_key, &ContextStateValue::from(index_slice))?;
                                    let _ = received_keys.insert(entry_state_key);
                                    let _ = received_keys.insert(index_state_key);
                                    applied += 1;

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
                                        // (S2.3: the side-store genesis floor seed
                                        // was removed — a cold joiner resolves an
                                        // anchor's writers from its stored
                                        // `metadata.storage_type.writers`, delivered
                                        // by this snapshot, when no rotation-log
                                        // collection is materialised yet.)
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
                                    // * `SYNC_STATE` — not written
                                    //   by the current codebase
                                    //   (grep `Key::SyncState`); a
                                    //   peer emitting one is
                                    //   misbehaving.
                                    // * `ROTATION_LOG` — per-entity
                                    //   writer-rotation history used
                                    //   by the verifier for
                                    //   `writers_at(causal_point)`.
                                    //   A forged rotation log would
                                    //   fool the verifier into
                                    //   accepting actions signed by
                                    //   writers who weren't
                                    //   authorized at the relevant
                                    //   causal point. The receiver
                                    //   reconstructs rotation
                                    //   history from verified delta
                                    //   replay; late-arriving
                                    //   pre-snapshot deltas that
                                    //   reference rotation points
                                    //   before the snapshot may fail
                                    //   to verify until per-entry
                                    //   rotation-log signing lands.
                                    //   Bounded edge case;
                                    //   acceptable trade-off for
                                    //   closing the trust gap.
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
                                        let mut handle = self.context_client.datastore_handle();
                                        for (id_obj, entry, index) in deferred_members.drain(..) {
                                            let metadata = match borsh::from_slice::<
                                                calimero_storage::index::EntityIndex,
                                            >(
                                                &index
                                            ) {
                                                Ok(idx) => idx.metadata,
                                                Err(e) => {
                                                    warn!(
                                                        %context_id,
                                                        id = ?id_obj.as_bytes(),
                                                        error = ?e,
                                                        "snapshot deferred SharedMember: index \
                                                         blob failed to deserialize — dropping"
                                                    );
                                                    continue;
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
                                                warn!(
                                                    %context_id,
                                                    id = ?id_obj.as_bytes(),
                                                    anchor = ?anchor.as_bytes(),
                                                    "snapshot deferred SharedMember: anchor not \
                                                     present in snapshot — dropping (member's \
                                                     writers unresolvable)"
                                                );
                                                continue;
                                            };
                                            if let Err(e) =
                                                Interface::<MainStorage>::verify_snapshot_member_signature(
                                                    id_obj, &entry, &metadata, writers,
                                                )
                                            {
                                                warn!(
                                                    %context_id,
                                                    id = ?id_obj.as_bytes(),
                                                    error = ?e,
                                                    "snapshot deferred SharedMember: signature \
                                                     verification failed — dropping"
                                                );
                                                continue;
                                            }
                                            let entry_state_key =
                                                StorageKey::Entry(id_obj).to_bytes();
                                            let index_state_key =
                                                StorageKey::Index(id_obj).to_bytes();
                                            let entry_key =
                                                ContextStateKey::new(context_id, entry_state_key);
                                            let index_key =
                                                ContextStateKey::new(context_id, index_state_key);
                                            let entry_slice: Slice<'_> = entry.into();
                                            let index_slice: Slice<'_> = index.into();
                                            handle.put(
                                                &entry_key,
                                                &ContextStateValue::from(entry_slice),
                                            )?;
                                            handle.put(
                                                &index_key,
                                                &ContextStateValue::from(index_slice),
                                            )?;
                                            let _ = received_keys.insert(entry_state_key);
                                            let _ = received_keys.insert(index_state_key);
                                            total_applied += 1;
                                        }
                                    }

                                    // All pages received - cleanup stale keys
                                    self.cleanup_stale_keys(
                                        context_id,
                                        &existing_keys,
                                        &received_keys,
                                    )?;
                                    return Ok(total_applied);
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

    /// Delete keys that existed before sync but weren't in the snapshot.
    fn cleanup_stale_keys(
        &self,
        context_id: ContextId,
        existing_keys: &std::collections::HashSet<[u8; 32]>,
        received_keys: &std::collections::HashSet<[u8; 32]>,
    ) -> Result<()> {
        let mut handle = self.context_client.datastore_handle();
        let mut deleted = 0;

        for state_key in existing_keys.difference(received_keys) {
            handle.delete(&ContextStateKey::new(context_id, *state_key))?;
            deleted += 1;
        }

        if deleted > 0 {
            debug!(%context_id, deleted, "Cleaned up stale keys");
        }
        Ok(())
    }

    /// Set a marker indicating snapshot sync is in progress for this context.
    ///
    /// This marker is used for crash recovery - if present on startup, the
    /// context's state may be inconsistent and needs to be re-synced.
    fn set_sync_in_progress_marker(
        &self,
        context_id: ContextId,
        boundary_root_hash: &Hash,
    ) -> Result<()> {
        use calimero_store::types::GenericData;

        let key = GenericKey::new(SYNC_IN_PROGRESS_SCOPE, *context_id);
        let value_bytes = borsh::to_vec(boundary_root_hash)?;
        let value: GenericData<'_> = Slice::from(value_bytes).into();
        let mut handle = self.context_client.datastore_handle();
        handle.put(&key, &value)?;
        debug!(%context_id, "Set sync-in-progress marker");
        Ok(())
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

    /// Clear the sync-in-progress marker after successful sync completion.
    /// Settle a context's per-context binding after a full-state snapshot.
    ///
    /// The snapshot adopts a peer's CURRENT state, so the context now runs the
    /// group's target schema: record the activation marker at the group's
    /// `app_key` and drop any stranded/resync markers. Running this for EVERY
    /// snapshot completion (not just resync) also gives a fresh joiner its
    /// first marker — without it the lazy gate would treat the just-synced
    /// state as stale and try to re-migrate it. Best-effort and group-only; a
    /// non-group context or zero `app_key` carries no version to bind.
    fn finalize_snapshot_activation(&self, context_id: ContextId) {
        settle_snapshot_activation(self.context_client.datastore(), context_id);
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

    /// Buffer a future-schema snapshot `Entity` into the absorb buffer instead
    /// of `handle.put`-storing unreadable bytes (PR-6b Task 6b.7).
    ///
    /// Keyed by the entity id (idempotent overwrite on re-delivery), under the
    /// *sender's* schema so the drain only re-verifies + persists it once this
    /// node advances to that reader. Caller has already confirmed
    /// `!snapshot_entity_is_readable(Some(schema), loaded)`.
    fn buffer_future_schema_snapshot_entity(
        &self,
        context_id: ContextId,
        id: [u8; 32],
        entry: &[u8],
        index: &[u8],
        schema: [u8; 32],
    ) -> Result<()> {
        let record = calimero_context::group_store::AbsorbRecord::from_snapshot_entity(
            id,
            entry.to_vec(),
            index.to_vec(),
            schema,
        );
        calimero_context::group_store::AbsorbRepository::new(self.context_client.datastore())
            .save(&context_id, schema, &record)?;
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
}

/// Outcome of draining a buffered snapshot entity (PR-6b Task 6b.7).
///
/// Distinguishes "this buffer record is finished — delete it" from "a transient
/// verify/parse failure — keep it for a later pass". The critical case is
/// [`SnapshotEntityDrainOutcome::RedrivenElsewhere`]: a `SharedMember` entity is
/// re-applied through the snapshot pass-2 anchor-authenticated path on the next
/// snapshot re-drive, NOT by this standalone drain. Once the loaded reader
/// matches its schema, leaving the buffer record pending would leak it forever
/// (the `drain_absorbed_leaves` early-exit keys on `entity.is_some()`, so every
/// later state-delta apply would rebuild a runtime env for the orphan). So that
/// case must be deleted too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotEntityDrainOutcome {
    /// Entry + Index blobs were re-verified and persisted — delete the record.
    Persisted,
    /// The entity is handled by the snapshot pass-2 re-drive (currently
    /// `SharedMember`, which needs anchor-writer authentication this standalone
    /// drain can't perform). The reader has advanced to its schema, so it WILL
    /// be re-applied through the full snapshot path — delete the orphaned buffer
    /// record so it stops blocking the drain early-exit.
    RedrivenElsewhere,
    /// A transient parse/verify failure — keep the record for a later pass.
    Pending,
}

/// Re-verify and persist a buffered future-schema snapshot entity (PR-6b Task
/// 6b.7), mirroring the inline persist in `request_and_apply_snapshot_pages`'s
/// `Entity` arm: parse the `index` blob as an `EntityIndex`, re-run
/// `verify_snapshot_entity_signature`, then `handle.put` the `entry` + `index`
/// blobs under their hashed storage keys. Returns
/// [`SnapshotEntityDrainOutcome::Pending`] on a malformed index or signature
/// failure; [`SnapshotEntityDrainOutcome::Persisted`] once persisted.
///
/// `SharedMember` entities — which the apply path defers to a second pass that
/// authenticates against anchor writer sets — are not re-derivable in this
/// standalone drain. The caller has already gated on the loaded reader matching
/// this entity's schema, so the member IS re-applied through the snapshot
/// pass-2 re-drive; we return [`SnapshotEntityDrainOutcome::RedrivenElsewhere`]
/// so the caller deletes the now-orphaned buffer record rather than leaking it.
pub(crate) fn persist_buffered_snapshot_entity(
    handle: &mut calimero_store::Handle<calimero_store::Store>,
    context_id: ContextId,
    id: [u8; 32],
    entry: &[u8],
    index: &[u8],
) -> Result<SnapshotEntityDrainOutcome> {
    let index_entity: calimero_storage::index::EntityIndex = match borsh::from_slice(index) {
        Ok(idx) => idx,
        Err(e) => {
            warn!(%context_id, id = ?id, error = ?e,
                "absorb entity drain: index blob failed to deserialize — leaving pending");
            return Ok(SnapshotEntityDrainOutcome::Pending);
        }
    };
    let id_obj = Id::new(id);

    if matches!(
        index_entity.metadata.storage_type,
        calimero_storage::entities::StorageType::SharedMember { .. }
    ) {
        warn!(%context_id, id = ?id,
            "absorb entity drain: SharedMember is re-applied by the snapshot pass-2 \
             re-drive (anchor-writer authentication) — deleting the orphaned buffer \
             record now the reader has advanced to its schema");
        return Ok(SnapshotEntityDrainOutcome::RedrivenElsewhere);
    }

    if let Err(e) = Interface::<MainStorage>::verify_snapshot_entity_signature(
        id_obj,
        entry,
        &index_entity.metadata,
    ) {
        warn!(%context_id, id = ?id, error = ?e,
            "absorb entity drain: signature verification failed — leaving pending");
        return Ok(SnapshotEntityDrainOutcome::Pending);
    }

    let entry_key = ContextStateKey::new(context_id, StorageKey::Entry(id_obj).to_bytes());
    let index_key = ContextStateKey::new(context_id, StorageKey::Index(id_obj).to_bytes());
    let entry_slice: Slice<'_> = entry.to_vec().into();
    let index_slice: Slice<'_> = index.to_vec().into();
    handle.put(&entry_key, &ContextStateValue::from(entry_slice))?;
    handle.put(&index_key, &ContextStateValue::from(index_slice))?;
    Ok(SnapshotEntityDrainOutcome::Persisted)
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
/// live store. That is safe because `stream_snapshot_pages`
/// re-reads `root_hash` after generation and rejects the snapshot
/// with `InvalidBoundary` if the context mutated in the meantime, so
/// a torn read can never be shipped to a peer.
fn generate_snapshot_pages<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
    start_cursor: Option<&SnapshotCursor>,
    page_limit: u16,
    byte_limit: u32,
    schema_app_key: Option<[u8; 32]>,
) -> Result<(Vec<Vec<u8>>, Option<SnapshotCursor>, u64)> {
    // Pass 1 — single snapshot scan, memory bounded to keys + ids.
    //
    // Retain only the 32-byte hashed state keys (`present_keys`,
    // for O(1) existence checks) and the discovered entity ids.
    // Record *values* are deserialized to identify `Index` records
    // and then dropped — never collected. The pre-#2133
    // implementation built a full `state_key → value` map here, so
    // a context with millions of keys pulled its entire state into
    // memory on every paginated call.
    let mut iter = handle.iter_snapshot::<ContextStateKey>()?;
    let mut present_keys: HashSet<[u8; 32]> = HashSet::new();
    let mut entity_ids: Vec<Id> = Vec::new();
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

        // Discover entity ids by trying to deserialize each value as
        // `EntityIndex`. Cross-check against the expected hashed key
        // (`Key::Index(id).to_bytes()`) to avoid false positives from
        // Entry values that happen to borsh-deserialize as a partial
        // EntityIndex. The borrowed value is dropped at the end of
        // the iteration — nothing about it is retained.
        if let Ok(index_entity) =
            borsh::from_slice::<calimero_storage::index::EntityIndex>(value.value.as_ref())
        {
            if StorageKey::Index(index_entity.id()).to_bytes() == state_key {
                entity_ids.push(index_entity.id());
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
    // **Note on `SyncState`:** the sender doesn't look up
    // `Key::SyncState(id)` — no production codebase path actually
    // writes that key (it's defined in the storage layer but
    // unused). The receiver mirrors this and rejects
    // `Auxiliary { kind: SYNC_STATE, .. }` as misbehaving. If
    // SyncState ever does start being written, replicating it
    // safely will require per-record authentication (it's not
    // bound to an entity signature) — track as a follow-up if /
    // when that need arises.
    //
    // Cursor support: skip any entity ids ≤ cursor.last_key. The
    // `≤` (not `<`) is correct because the cursor records the
    // last fully-committed entity, not "next to emit."
    let start_after_id = start_cursor.map(|c| c.last_key);
    let mut consumed_keys: HashSet<[u8; 32]> = HashSet::new();
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
        let entry_key = StorageKey::Entry(*id).to_bytes();
        let has_index = present_keys.contains(&index_key);
        let has_entry = present_keys.contains(&entry_key);

        // An entity contributes 1 record (Entity bundling Entry +
        // Index). The writer-set rotation log is no longer a separate
        // auxiliary key — it lives as `UnorderedMap` collection
        // children, each shipped as its own Entry+Index entity above.
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
                if has_entry {
                    let _ = consumed_keys.insert(entry_key);
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
                let _ = consumed_keys.insert(entry_key);
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

        // The writer-set rotation log is no longer a separate
        // auxiliary key — it lives as `UnorderedMap` collection
        // children, each a normal Entry+Index entity classified and
        // shipped by the loop above. Nothing extra to consume here.
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
        let entry_key = StorageKey::Entry(*id).to_bytes();
        // Only fully-paired entities are shipped; orphans were
        // diagnosed in the accounting pass and are intentionally
        // dropped. The existence pre-check avoids a value lookup for
        // ids that can't produce a bundle.
        if !(present_keys.contains(&index_key) && present_keys.contains(&entry_key)) {
            continue;
        }

        // Materialise the values now. A `None` means the record was
        // removed between the Pass-1 scan and this live-store lookup
        // (a concurrent delete). The post-generation `root_hash`
        // recheck rejects the whole snapshot when state changed, so
        // skipping the entity here is safe — but log it so the
        // otherwise-silent skip is observable if it ever fires.
        let Some(index) = handle.get(&ContextStateKey::new(context_id, index_key))? else {
            warn!(
                %context_id, id = ?id_bytes,
                "snapshot emit: Index value vanished between scan and read \
                 (concurrent delete?) — skipping entity; root_hash recheck guards correctness"
            );
            continue;
        };
        let index = index.value.to_vec();
        let Some(entry) = handle.get(&ContextStateKey::new(context_id, entry_key))? else {
            warn!(
                %context_id, id = ?id_bytes,
                "snapshot emit: Entry value vanished between scan and read \
                 (concurrent delete?) — skipping entity; root_hash recheck guards correctness"
            );
            continue;
        };
        let entry = entry.value.to_vec();

        // V2 page format (PR-6b): each record is length-framed
        // (`u32 LE len ‖ record_bytes`) so the receiver bounds each
        // record's decode to its own sub-slice. Without framing, the
        // backward-compatible trailing `Entity.schema_app_key` (decoded
        // EOF-tolerantly) would, on a record that is NOT the last in the
        // page, read the next record's leading bytes instead of seeing a
        // clean EOF — desyncing the whole page.
        let record_bytes = encode_framed_snapshot_record(&SnapshotRecord::Entity {
            id: id_bytes,
            entry,
            index,
            schema_app_key,
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
/// EOF-tolerant trailing `Entity.schema_app_key` sound even for a non-terminal
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
///   sub-slice, so the EOF-tolerant trailing `Entity.schema_app_key` reads a
///   clean EOF at the sub-slice boundary instead of bleeding into the next
///   record — correct even for a non-terminal record.
/// * **legacy** (pre-#2539) — the page is a back-to-back concatenation of
///   borsh-encoded records with NO framing and NO trailing `schema_app_key`
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
        // end is what makes the EOF-tolerant trailing `schema_app_key` sound.
        let record = borsh::from_slice::<SnapshotRecord>(body)?;
        records.push(record);
        remaining = &remaining[body_end..];
    }
    Ok(records)
}

/// Decode a **legacy** (pre-#2539) unframed page: records concatenated
/// end-to-end with no trailing `schema_app_key`. Each record is self-delimiting
/// (borsh `Vec` fields carry their own length), so we decode sequentially —
/// but we must NOT peek for the trailing `Entity.schema_app_key` byte, because
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
                schema_app_key: None,
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
) -> Result<Vec<[u8; 32]>> {
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

/// Settle a context's per-context binding after a full-state snapshot.
///
/// The snapshot adopts a peer's CURRENT state, so the context now runs the
/// group's target schema: record the activation marker at the group's
/// `app_key` and drop any stranded/resync markers. Run for EVERY snapshot
/// completion (not just resync) so a fresh joiner also gets its first marker —
/// without it the lazy gate would treat the just-synced state as stale and try
/// to re-migrate it. Best-effort and group-only; a non-group context or zero
/// `app_key` carries no version to bind.
fn settle_snapshot_activation(store: &calimero_store::Store, context_id: ContextId) {
    use calimero_context::group_store::{get_group_for_context, MetaRepository};

    let app_key = match get_group_for_context(store, &context_id) {
        Ok(Some(gid)) => match MetaRepository::new(store).load(&gid) {
            Ok(Some(meta)) if meta.app_key != [0u8; 32] => meta.app_key,
            _ => return,
        },
        _ => return,
    };

    calimero_context::activation::record_activation(store, &context_id, app_key);
    let mut handle = store.handle();
    let _ = handle.delete(&calimero_store::key::ContextMigrationFailed::new(
        context_id.into(),
    ));
    let _ = handle.delete(&calimero_store::key::ContextResyncRequested::new(
        context_id.into(),
    ));
    debug!(%context_id, "snapshot activation settled; markers cleared");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_primitives::context::ContextId;
    use calimero_storage::index::EntityIndex;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    // Wire-codec round-trip tests below use the `ROTATION_LOG` auxiliary
    // kind as a sample `SnapshotRecord::Auxiliary`; the constant lives in
    // node-primitives and is exercised only here (the sender never emits a
    // rotation-log auxiliary record — the log syncs as collection children).
    use calimero_node_primitives::sync::snapshot::snapshot_record_kind;

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

    /// Persist a well-formed entity (Index + Entry pair) for `ctx`
    /// into `store`, mirroring how production state is laid out: the
    /// Index value is a borsh-serialized `EntityIndex` whose id
    /// hashes to the Index state-key.
    fn put_entity(store: &Store, ctx: ContextId, id_bytes: [u8; 32], entry_len: usize) {
        let id = Id::new(id_bytes);
        let index_bytes = borsh::to_vec(&EntityIndex::minimal_for_test(id)).unwrap();
        // Entry payload is opaque to the sender; fill it with a
        // recognizable byte so size-based pagination has something to
        // chew on. It must NOT deserialize as an `EntityIndex` at the
        // Entry state-key, which is guaranteed by the key cross-check
        // in discovery regardless of the bytes here.
        let entry_bytes = vec![0xEE_u8; entry_len];

        let mut handle = store.handle();
        let index_key = ContextStateKey::new(ctx, StorageKey::Index(id).to_bytes());
        handle
            .put(
                &index_key,
                &ContextStateValue::from(Slice::from(index_bytes)),
            )
            .unwrap();
        let entry_key = ContextStateKey::new(ctx, StorageKey::Entry(id).to_bytes());
        handle
            .put(
                &entry_key,
                &ContextStateValue::from(Slice::from(entry_bytes)),
            )
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
        let orphan_bytes = borsh::to_vec(&EntityIndex::minimal_for_test(orphan_id)).unwrap();
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

    /// Regression (PR-6b Task 6b.7 review): once the loaded reader matches a
    /// buffered `SharedMember` entity's schema, the standalone drain can't
    /// re-apply it (it needs snapshot pass-2 anchor-writer authentication) — but
    /// the member IS re-applied by the snapshot re-drive. So the drain must
    /// signal `RedrivenElsewhere` (delete the orphaned buffer record) rather
    /// than `Pending` (leak it forever, blocking the drain early-exit).
    #[test]
    fn test_persist_buffered_snapshot_entity_sharedmember_is_redriven_not_pending() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut handle = store.handle();
        let ctx = ContextId::from([5u8; 32]);
        let id = [6u8; 32];

        // Build a SharedMember index blob.
        let mut index = EntityIndex::minimal_for_test(Id::new(id));
        index.metadata.storage_type = calimero_storage::entities::StorageType::SharedMember {
            anchor: Id::new([7u8; 32]),
            signature_data: None,
        };
        let index_bytes = borsh::to_vec(&index).unwrap();

        let outcome =
            persist_buffered_snapshot_entity(&mut handle, ctx, id, &[1, 2, 3], &index_bytes)
                .unwrap();
        assert_eq!(
            outcome,
            SnapshotEntityDrainOutcome::RedrivenElsewhere,
            "a matching-schema SharedMember must be marked redriven (delete), not left pending"
        );

        // A malformed index blob is still a transient Pending (kept for retry).
        let pending =
            persist_buffered_snapshot_entity(&mut handle, ctx, id, &[1], &[0xFF, 0xFF]).unwrap();
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
            schema_app_key: None,
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
    /// `schema_app_key` byte and NO per-record length framing. A v2 reader
    /// must decode every such record with `schema_app_key == None` and consume
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
                schema_app_key: None,
            }
        );
        assert_eq!(
            records[1],
            SnapshotRecord::Entity {
                id: [2u8; 32],
                entry: vec![60],
                index: vec![70, 80, 90],
                schema_app_key: None,
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
                schema_app_key: Some([7u8; 32]),
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
                schema_app_key: Some([7u8; 32]),
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
            schema_app_key: None,
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
    fn settle_snapshot_activation_binds_to_group_app_key_and_clears_markers() {
        use calimero_context::group_store::{register_context_in_group, MetaRepository};
        use calimero_context_config::types::ContextGroupId;
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::UpgradePolicy;
        use calimero_primitives::identity::PublicKey;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key;
        use calimero_store::types::ContextMigrationFailed;
        use calimero_store::Store;

        const APP_KEY: [u8; 32] = [0x2A; 32];
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let gid = ContextGroupId::from([0x60; 32]);
        let ctx = ContextId::from([0x50; 32]);

        MetaRepository::new(&store)
            .save(
                &gid,
                &key::GroupMetaValue {
                    app_key: APP_KEY,
                    target_application_id: ApplicationId::from([0xAA; 32]),
                    upgrade_policy: UpgradePolicy::LazyOnAccess,
                    created_at: 0,
                    admin_identity: PublicKey::from([0x07; 32]),
                    owner_identity: PublicKey::from([0x07; 32]),
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
                &key::ContextMigrationFailed::new(ctx.into()),
                &ContextMigrationFailed { kind: 3 },
            )
            .unwrap();
        handle
            .put(&key::ContextResyncRequested::new(ctx.into()), &())
            .unwrap();
        drop(handle);

        settle_snapshot_activation(&store, ctx);

        assert_eq!(
            calimero_context::activation::activated_blob(&store, &ctx),
            Some(APP_KEY),
            "activation marker must bind to the group's current app_key"
        );
        let handle = store.handle();
        assert!(
            handle
                .get(&key::ContextMigrationFailed::new(ctx.into()))
                .unwrap()
                .is_none(),
            "stranded marker must be cleared"
        );
        assert!(
            handle
                .get(&key::ContextResyncRequested::new(ctx.into()))
                .unwrap()
                .is_none(),
            "resync marker must be cleared"
        );
    }

    #[test]
    fn settle_snapshot_activation_noop_for_non_group_context() {
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([0x51; 32]);
        // No group mapping → nothing to bind, no panic, no marker written.
        settle_snapshot_activation(&store, ctx);
        assert_eq!(
            calimero_context::activation::activated_blob(&store, &ctx),
            None
        );
    }
}
