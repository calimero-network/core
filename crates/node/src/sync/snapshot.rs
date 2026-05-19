//! Snapshot sync protocol for full state bootstrap.

use std::collections::{BTreeMap, HashSet};

use borsh::BorshDeserialize;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::snapshot::{snapshot_record_kind, SnapshotRecord};
use calimero_node_primitives::sync::{
    MessagePayload, SnapshotCursor, SnapshotError, StreamMessage,
};
use calimero_primitives::context::ContextId;
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

/// Scope for sync-in-progress markers in the Generic column.
/// Exactly 16 bytes to match SCOPE_SIZE.
const SYNC_IN_PROGRESS_SCOPE: [u8; SCOPE_SIZE] = *b"sync-in-progres\0";

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
        let (pages, next_cursor, total_entries) = generate_snapshot_pages(
            &handle,
            context_id,
            start_cursor.as_ref(),
            page_limit,
            byte_limit,
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
        if !force && !is_crash_recovery {
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

        let mut stream = self.network_client.open_stream(peer_id).await?;
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
                    } => {
                        // Handle empty snapshot (no entries)
                        if payload.is_empty() && uncompressed_len == 0 {
                            // Empty snapshot - delete all existing keys
                            self.cleanup_stale_keys(context_id, &existing_keys, &received_keys)?;
                            return Ok(total_applied);
                        }

                        let decompressed = lz4_flex::decompress_size_prepended(&payload)
                            .map_err(|e| eyre::eyre!("Decompress failed: {}", e))?;

                        if decompressed.len() != uncompressed_len as usize {
                            eyre::bail!(
                                "Size mismatch: {} vs {}",
                                uncompressed_len,
                                decompressed.len()
                            );
                        }

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
                                SnapshotRecord::Entity { id, entry, index } => {
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
                                    // Preserve any local RotationLog
                                    // history for this entity from
                                    // the upcoming `cleanup_stale_keys`
                                    // pass — snapshot doesn't ship
                                    // rotation logs but the receiver
                                    // may have built one up via
                                    // verified delta replay.
                                    let _ = received_keys
                                        .insert(StorageKey::RotationLog(id_obj).to_bytes());
                                    applied += 1;
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

                        // Check if this is the last page in this burst
                        let is_last_in_burst = sent_count == page_count;

                        if is_last_in_burst {
                            // Check if there are more pages to fetch
                            match cursor {
                                None => {
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

    /// Clear the sync-in-progress marker after successful sync completion.
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

/// Generate snapshot pages. Returns (pages, next_cursor, total_entries).
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
/// 1. Iterate all `ContextStateKey` records for this context.
/// 2. For each record value, attempt borsh deserialization as
///    [`calimero_storage::index::EntityIndex`]; success identifies
///    the record as `Key::Index(id)` and yields the entity id.
/// 3. For each discovered id, look up `Key::Entry(id)` and
///    `Key::RotationLog(id)` records by their hashed state keys
///    and bundle them.
/// 4. Records whose state-key doesn't match any discovered
///    `Index`/`Entry`/`RotationLog` slot are dropped as orphans
///    (with a warning) — a well-formed state tree shouldn't have
///    any.
fn generate_snapshot_pages<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
    start_cursor: Option<&SnapshotCursor>,
    page_limit: u16,
    byte_limit: u32,
) -> Result<(Vec<Vec<u8>>, Option<SnapshotCursor>, u64)> {
    // Use snapshot iterator for consistent reads during iteration
    let mut iter = handle.iter_snapshot::<ContextStateKey>()?;

    // Collect entries for this context into a state_key → value map
    // for O(1) lookup by hashed key.
    let mut all_records: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
    for (key_result, value_result) in iter.entries() {
        let key = key_result?;
        let value = value_result?;
        if key.context_id() == context_id {
            let _ = all_records.insert(key.state_key(), value.value.to_vec());
        }
    }
    let total_records = all_records.len();

    // Discover entity ids by trying to deserialize each value as
    // `EntityIndex`. Cross-check against the expected hashed key
    // (`Key::Index(id).to_bytes()`) to avoid false positives from
    // Entry values that happen to borsh-deserialize as a partial
    // EntityIndex.
    //
    // `consumed_keys` is intentionally populated ONLY in the
    // bundling loop below — never here. That way the
    // `unrecognized_count` at the end captures *every* state_key
    // we didn't actually emit on the wire, and the orphan
    // counters can be inspected as a subdivision of that total
    // (a state_key contributes both to its specific orphan
    // counter AND to `unrecognized_count`, which is the intended
    // operator-visible behavior).
    let mut entity_ids: Vec<Id> = Vec::new();
    let mut consumed_keys: HashSet<[u8; 32]> = HashSet::new();
    for (state_key, value) in &all_records {
        let Ok(index_entity) =
            borsh::from_slice::<calimero_storage::index::EntityIndex>(value.as_slice())
        else {
            continue;
        };
        let expected = StorageKey::Index(index_entity.id()).to_bytes();
        if expected == *state_key {
            entity_ids.push(index_entity.id());
        }
    }
    entity_ids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    // Build per-entity bundles in canonical (id-sorted) order. Each
    // bundle holds all `SnapshotRecord`s for one entity (the
    // `Entity` Entry+Index pair plus its optional
    // `Auxiliary::ROTATION_LOG`). Pagination is atomic on bundle
    // boundaries — a bundle either fits entirely on the current
    // page or moves to the next. Without this, the per-record
    // pagination could split an entity across pages, and the
    // cursor (which records the last entity id committed) would
    // cause the resumed iteration to skip the entity outright,
    // permanently dropping its Auxiliary records.
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
    let mut bundles: Vec<([u8; 32], Vec<SnapshotRecord>)> = Vec::new();
    // Specific anomaly counters. They subdivide `unrecognized_count`
    // computed below — a state_key flagged here is ALSO counted in
    // the residual unrecognized total. That's intentional: ops can
    // see "100 non-bundle records dropped, of which 95 were orphan
    // Indexes (specific pattern), 5 were truly unrecognized."
    let mut orphan_index_without_entry: u64 = 0;
    let mut orphan_entry_without_index: u64 = 0;
    for id in &entity_ids {
        let id_bytes = *id.as_bytes();
        let index_key = StorageKey::Index(*id).to_bytes();
        let entry_key = StorageKey::Entry(*id).to_bytes();
        let rotation_log_key = StorageKey::RotationLog(*id).to_bytes();
        if let Some(after) = start_after_id {
            if id_bytes <= after {
                // Cursor-skipped — these keys were already shipped on
                // a prior page. Mark them consumed so they don't
                // appear in the residual unrecognized count for the
                // current page.
                //
                // Only insert keys that actually exist in
                // `all_records`. Inserting a phantom key (e.g. a
                // RotationLog state_key for an entity that doesn't
                // have one) would push `consumed_keys.len()` past
                // `total_records`, making the
                // `saturating_sub` at the bottom return 0 and
                // silently suppress the operator-visible
                // unrecognized-records warning even when real
                // orphans exist.
                if all_records.contains_key(&index_key) {
                    let _ = consumed_keys.insert(index_key);
                }
                if all_records.contains_key(&entry_key) {
                    let _ = consumed_keys.insert(entry_key);
                }
                if all_records.contains_key(&rotation_log_key) {
                    let _ = consumed_keys.insert(rotation_log_key);
                }
                continue;
            }
        }

        let index_bytes = all_records.get(&index_key).cloned();
        let entry_bytes = all_records.get(&entry_key).cloned();
        let rotation_log_bytes = all_records.get(&rotation_log_key).cloned();

        let mut bundle: Vec<SnapshotRecord> = Vec::new();

        // Emit Entity record bundling Entry + Index (the common case
        // — every persisted entity has both). Verification on the
        // receiver runs against the metadata inside `index`.
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
        // shouldn't exist anyway; if they do we drop them (debug
        // log emitted below) rather than ship them on an unverified
        // channel.
        //
        // `consumed_keys` is updated only on successful bundling /
        // explicit cursor skip. Orphan arms intentionally do NOT
        // insert the orphan key into `consumed_keys` so they flow
        // into the final `unrecognized_count` (the operator-visible
        // catch-all for non-bundle records).
        match (index_bytes.clone(), entry_bytes.clone()) {
            (Some(index), Some(entry)) => {
                bundle.push(SnapshotRecord::Entity {
                    id: id_bytes,
                    entry,
                    index,
                });
                let _ = consumed_keys.insert(index_key);
                let _ = consumed_keys.insert(entry_key);
            }
            (Some(_), None) => {
                debug!(
                    %context_id, id = ?id_bytes,
                    "dropping orphan Index (no matching Entry) — would be \
                     unverifiable on the receiver"
                );
                orphan_index_without_entry += 1;
            }
            (None, Some(_)) => {
                // Structurally unreachable: `entity_ids` was derived
                // from successful `EntityIndex` deserializations in
                // `all_records`, so `index_bytes` is always `Some`
                // here. Kept for exhaustiveness; if it ever does
                // fire it indicates a discovery bug and we want it
                // counted as an orphan.
                debug!(
                    %context_id, id = ?id_bytes,
                    "unreachable: entity_id without matching Index in all_records"
                );
                orphan_entry_without_index += 1;
            }
            (None, None) => {}
        }

        // RotationLog is intentionally NOT shipped via snapshot.
        // The receiver rejects all `SnapshotRecord::Auxiliary`
        // kinds (issue #2387 follow-up: per-record signing).
        // Sending them would just burn bandwidth for records the
        // receiver drops. The receiver reconstructs rotation
        // history from verified delta replay; late-arriving
        // pre-snapshot deltas referencing pre-snapshot rotation
        // points may fail to verify until per-entry rotation-log
        // signing lands — bounded edge case, documented in the
        // receiver's Auxiliary reject path.
        if rotation_log_bytes.is_some() {
            // Mark the key consumed so the unrecognized-records
            // warning at the bottom doesn't fire for it (we know
            // about the record; we're choosing not to ship it).
            let _ = consumed_keys.insert(rotation_log_key);
        }

        if !bundle.is_empty() {
            bundles.push((id_bytes, bundle));
        }
    }

    // Residual non-bundle records: state_keys present in
    // `all_records` that weren't bundled and weren't cursor-skipped.
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

    // Count of records that would be emitted in a fresh (no-cursor)
    // run — counts every entity's bundle, regardless of whether
    // we're cursor-skipping it on this call. Stable across paginated
    // calls so operators can monitor snapshot progress reliably
    // (this value flows into the "Streaming snapshot" info log; the
    // bot review pointed out that the old per-page-bundles count
    // shrank with each paginated call, making the log misleading).
    let total_entries: u64 = {
        let mut count: u64 = 0;
        for id in &entity_ids {
            let index_key = StorageKey::Index(*id).to_bytes();
            let entry_key = StorageKey::Entry(*id).to_bytes();
            // An entity contributes 1 record (Entity bundling Entry +
            // Index) — RotationLog Auxiliary isn't shipped per the
            // #2387 security trade-off, so it doesn't contribute.
            if all_records.contains_key(&index_key) && all_records.contains_key(&entry_key) {
                count += 1;
            }
        }
        count
    };

    // Serialize bundles into pages atomically. Cursor records the
    // last entity id whose bundle was fully committed to a page.
    //
    // **Invariant**: `last_id` is `Some` whenever the early-return
    // path inside the loop fires. The early-return is gated on
    // `pages.len() >= page_limit`, which only increases after a
    // `pages.push(current_page)`; that push requires
    // `current_page` to be non-empty, which only happens after a
    // prior bundle's `current_page.extend(bundle_bytes)` — and
    // that extend sets `last_id = Some(bundle_id)`. So the cursor
    // emitted on early-return always references a real, fully-
    // committed entity id; we never accidentally signal completion
    // (cursor = None) when more bundles remain.
    let mut pages: Vec<Vec<u8>> = Vec::new();
    let mut current_page: Vec<u8> = Vec::new();
    let mut last_id: Option<[u8; 32]> = None;

    for (bundle_id, bundle) in bundles.into_iter() {
        // Pre-serialize the whole bundle so we know its size.
        let mut bundle_bytes: Vec<u8> = Vec::new();
        for record in &bundle {
            let record_bytes = borsh::to_vec(record)?;
            bundle_bytes.extend(record_bytes);
        }

        // Page-break BEFORE adding this bundle if it would exceed
        // byte_limit and the current page isn't empty. A bundle that
        // by itself exceeds byte_limit still goes on its own page —
        // splitting would defeat atomicity, and oversized bundles
        // are bounded by `MAX_ENTITY_DATA_SIZE` on the wire types.
        if !current_page.is_empty() && (current_page.len() + bundle_bytes.len()) as u32 > byte_limit
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

        current_page.extend(bundle_bytes);
        last_id = Some(bundle_id);
    }

    if !current_page.is_empty() {
        pages.push(current_page);
    }

    Ok((pages, None, total_entries))
}

/// Decode snapshot records from a (decompressed) page payload.
///
/// Mirrors the encoding in [`generate_snapshot_pages`]: borsh-encoded
/// [`SnapshotRecord`]s concatenated end-to-end. Used by both the
/// receiver and the snapshot test fixtures.
fn decode_snapshot_records(payload: &[u8]) -> Result<Vec<SnapshotRecord>> {
    let mut records = Vec::new();
    let mut remaining = payload;
    while !remaining.is_empty() {
        let mut cursor = remaining;
        let record = SnapshotRecord::deserialize(&mut cursor)?;
        let consumed = remaining.len() - cursor.len();
        if consumed == 0 {
            eyre::bail!("snapshot record deserialization made no progress");
        }
        remaining = cursor;
        records.push(record);
    }

    Ok(records)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let encoded = borsh::to_vec(&record).unwrap();

        let records = decode_snapshot_records(&encoded).unwrap();
        assert_eq!(records.len(), 1);
        match &records[0] {
            SnapshotRecord::Entity { id, entry, index } => {
                assert_eq!(*id, [1u8; 32]);
                assert_eq!(entry, &vec![10, 20, 30]);
                assert_eq!(index, &vec![40, 50, 60]);
            }
            _ => panic!("expected Entity record"),
        }
    }

    #[test]
    fn test_decode_snapshot_records_mixed() {
        // Mix Entity + Auxiliary records in a single page payload to
        // exercise the streaming decode boundary handling.
        let entity = SnapshotRecord::Entity {
            id: [1u8; 32],
            entry: vec![10],
            index: vec![20],
        };
        let aux = SnapshotRecord::Auxiliary {
            kind: snapshot_record_kind::ROTATION_LOG,
            id: [2u8; 32],
            value: vec![30, 31],
        };

        let mut encoded = borsh::to_vec(&entity).unwrap();
        encoded.extend(borsh::to_vec(&aux).unwrap());

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
}
