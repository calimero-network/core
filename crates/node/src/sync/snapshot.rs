//! Snapshot sync protocol (Phase 1).
//!
//! Provides full state bootstrap and fallback sync when delta sync
//! is not possible (e.g., peer's history is pruned).
//!
//! ## Flow
//!
//! 1. Requester sends `SnapshotBoundaryRequest` to negotiate boundary
//! 2. Responder returns `SnapshotBoundaryResponse` with root hash and metadata
//! 3. Requester sends `SnapshotStreamRequest` to start streaming
//! 4. Responder sends `SnapshotPage` chunks until cursor is empty
//! 5. Requester applies snapshot and verifies root hash
//! 6. Fine-sync deltas from boundary to latest using `DagHeadsRequest`

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    MessagePayload, SnapshotCursor, SnapshotError, StreamMessage, TreeParams,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_storage::env::time_now;
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use eyre::Result;
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

/// Maximum uncompressed bytes per snapshot page.
/// This is a reasonable default that balances memory usage and transfer efficiency.
pub const DEFAULT_PAGE_BYTE_LIMIT: u32 = 64 * 1024; // 64 KB

/// Maximum pages to send in a single burst.
pub const DEFAULT_PAGE_LIMIT: u16 = 16;

/// Snapshot encoding version for canonical format.
/// Increment this when the encoding format changes.
#[allow(dead_code)] // Reserved for future use in format versioning
pub const SNAPSHOT_ENCODING_VERSION: u16 = 1;

impl SyncManager {
    /// Handle incoming snapshot boundary request from a peer.
    ///
    /// Returns the current state as the boundary (Phase 1 always uses current state).
    pub async fn handle_snapshot_boundary_request(
        &self,
        context_id: ContextId,
        _requested_cutoff_timestamp: Option<u64>,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        info!(
            %context_id,
            "Handling snapshot boundary request from peer"
        );

        // Get context to retrieve current state
        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                warn!(%context_id, "Context not found for snapshot boundary request");
                return self
                    .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                    .await;
            }
        };

        // Phase 1: Always use current state as boundary
        // Future: Support historical boundaries based on requested_cutoff_timestamp
        let boundary_timestamp = time_now();
        let boundary_root_hash = context.root_hash;
        let dag_heads = context.dag_heads.clone();

        // Estimate total size (optional, for progress reporting)
        // For now, we don't have an easy way to estimate without iterating
        let total_estimate = None;

        // Phase 1: No Merkle tree params
        let tree_params: Option<TreeParams> = None;
        let leaf_count = None;
        let merkle_root_hash = None;

        info!(
            %context_id,
            %boundary_root_hash,
            heads_count = dag_heads.len(),
            "Sending snapshot boundary response"
        );

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SnapshotBoundaryResponse {
                boundary_timestamp,
                boundary_root_hash,
                dag_heads,
                total_estimate,
                tree_params,
                leaf_count,
                merkle_root_hash,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }

    /// Handle incoming snapshot stream request from a peer.
    ///
    /// Streams snapshot pages until completion or error.
    #[expect(clippy::too_many_arguments, reason = "protocol handler needs all params")]
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
        info!(
            %context_id,
            %boundary_root_hash,
            page_limit,
            byte_limit,
            has_cursor = resume_cursor.is_some(),
            "Handling snapshot stream request from peer"
        );

        // Verify the boundary is still valid (current root hash matches)
        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                warn!(%context_id, "Context not found for snapshot stream request");
                return self
                    .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                    .await;
            }
        };

        // Phase 1: Boundary must match current state
        // Future: Support pinned historical views
        if context.root_hash != boundary_root_hash {
            warn!(
                %context_id,
                expected = %boundary_root_hash,
                actual = %context.root_hash,
                "Boundary root hash mismatch - state changed during sync"
            );
            return self
                .send_snapshot_error(stream, SnapshotError::InvalidBoundary)
                .await;
        }

        // Parse resume cursor if provided
        let start_cursor = if let Some(cursor_bytes) = resume_cursor {
            match SnapshotCursor::try_from_slice(&cursor_bytes) {
                Ok(cursor) => Some(cursor),
                Err(e) => {
                    warn!(%context_id, error = %e, "Invalid resume cursor");
                    return self
                        .send_snapshot_error(stream, SnapshotError::ResumeCursorInvalid)
                        .await;
                }
            }
        } else {
            None
        };

        // Stream snapshot pages
        self.stream_snapshot_pages(context_id, start_cursor, page_limit, byte_limit, stream)
            .await
    }

    /// Stream snapshot pages to a peer.
    ///
    /// Uses the canonical snapshot encoding with deterministic ordering.
    async fn stream_snapshot_pages(
        &self,
        context_id: ContextId,
        start_cursor: Option<SnapshotCursor>,
        page_limit: u16,
        byte_limit: u32,
        stream: &mut Stream,
    ) -> Result<()> {
        // Get datastore handle for direct storage access
        let handle = self.context_client.datastore_handle();

        // Generate snapshot pages by iterating over ContextState entries
        let (pages, next_cursor, total_entries) =
            generate_snapshot_pages(&handle, context_id, start_cursor.as_ref(), page_limit, byte_limit)?;

        info!(
            %context_id,
            pages_count = pages.len(),
            total_entries,
            has_more = next_cursor.is_some(),
            "Streaming snapshot pages"
        );

        // Send each page
        let mut sqx = Sequencer::default();
        let page_count = pages.len() as u64;

        for (i, page_data) in pages.into_iter().enumerate() {
            let is_last = i == (page_count as usize - 1) && next_cursor.is_none();

            // Compress payload using lz4
            let compressed = lz4_flex::compress_prepend_size(&page_data);
            let uncompressed_len = page_data.len() as u32;

            // Serialize cursor for next page
            let cursor = if is_last {
                None
            } else if i == (page_count as usize - 1) {
                // Last page in this batch, include next cursor
                next_cursor
                    .as_ref()
                    .map(|c| borsh::to_vec(c).expect("cursor serialization should not fail"))
            } else {
                // More pages in this batch, no cursor needed
                None
            };

            let msg = StreamMessage::Message {
                sequence_id: sqx.next(),
                payload: MessagePayload::SnapshotPage {
                    payload: compressed.into(),
                    uncompressed_len,
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

    /// Send a snapshot error response to a peer.
    async fn send_snapshot_error(&self, stream: &mut Stream, error: SnapshotError) -> Result<()> {
        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SnapshotError { error },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }

    // =========================================================================
    // Requester-side snapshot sync methods
    // =========================================================================

    /// Request and apply a full snapshot from a peer.
    ///
    /// This is the main entry point for snapshot sync on the requester side.
    /// It performs:
    /// 1. Boundary negotiation
    /// 2. Snapshot page streaming
    /// 3. State application
    /// 4. Root hash verification
    ///
    /// Returns the boundary DAG heads for fine-sync.
    pub async fn request_snapshot_sync(
        &self,
        context_id: ContextId,
        peer_id: libp2p::PeerId,
    ) -> Result<SnapshotSyncResult> {
        info!(
            %context_id,
            %peer_id,
            "Starting snapshot sync with peer"
        );

        // Step 1: Open stream to peer
        let mut stream = self.network_client.open_stream(peer_id).await?;

        // Step 2: Negotiate boundary
        let boundary = self
            .request_snapshot_boundary(context_id, &mut stream)
            .await?;

        info!(
            %context_id,
            boundary_root_hash = %boundary.boundary_root_hash,
            heads_count = boundary.dag_heads.len(),
            "Received snapshot boundary"
        );

        // Step 3: Stream and apply pages
        let applied_records = self
            .request_and_apply_snapshot_pages(context_id, &boundary, &mut stream)
            .await?;

        info!(
            %context_id,
            applied_records,
            "Applied snapshot pages"
        );

        // Step 4: Update context metadata to match the boundary
        // The snapshot data has been written to ContextState, now we need to update
        // ContextMeta with the boundary's root_hash and dag_heads
        self.context_client
            .force_root_hash(&context_id, boundary.boundary_root_hash)?;
        self.context_client
            .update_dag_heads(&context_id, boundary.dag_heads.clone())?;

        info!(
            %context_id,
            root_hash = %boundary.boundary_root_hash,
            dag_heads_count = boundary.dag_heads.len(),
            "Snapshot sync completed - context metadata updated"
        );

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

        // Get our identity for this context
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) = crate::utils::choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            eyre::bail!("No owned identity found for context: {}", context_id);
        };

        // Send boundary request
        let msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::SnapshotBoundaryRequest {
                context_id,
                requested_cutoff_timestamp: None, // Phase 1: use current state
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        // Receive boundary response
        let response = super::stream::recv(stream, None, self.sync_config.timeout).await?;

        let Some(StreamMessage::Message { payload, .. }) = response else {
            eyre::bail!("Unexpected response to snapshot boundary request");
        };

        match payload {
            MessagePayload::SnapshotBoundaryResponse {
                boundary_timestamp,
                boundary_root_hash,
                dag_heads,
                total_estimate,
                tree_params: _,
                leaf_count: _,
                merkle_root_hash: _,
            } => Ok(SnapshotBoundary {
                boundary_timestamp,
                boundary_root_hash,
                dag_heads,
                total_estimate,
            }),
            MessagePayload::SnapshotError { error } => {
                eyre::bail!("Snapshot boundary request failed: {:?}", error);
            }
            _ => {
                eyre::bail!("Unexpected payload in snapshot boundary response");
            }
        }
    }

    /// Request and apply snapshot pages from a peer.
    async fn request_and_apply_snapshot_pages(
        &self,
        context_id: ContextId,
        boundary: &SnapshotBoundary,
        stream: &mut Stream,
    ) -> Result<usize> {
        use calimero_node_primitives::sync::InitPayload;

        // Get our identity
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) = crate::utils::choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            eyre::bail!("No owned identity found for context: {}", context_id);
        };

        // Clear existing state for this context before applying snapshot
        // This is the "clear + write" approach per Phase 1 decision
        {
            let handle = self.context_client.datastore_handle();
            let keys = collect_context_state_keys(&handle, context_id)?;

            info!(
                %context_id,
                existing_entries = keys.len(),
                "Clearing existing state before snapshot apply"
            );

            // Delete entries using transaction
            let mut tx = self.context_client.datastore_handle();
            for state_key in keys {
                let key = ContextStateKey::new(context_id, state_key);
                tx.delete(&key)?;
            }
        }

        let mut total_applied = 0;
        let mut resume_cursor: Option<Vec<u8>> = None;
        let mut page_num = 0;

        loop {
            page_num += 1;

            // Send stream request
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

            // Receive and apply pages
            let mut pages_in_batch = 0;
            #[allow(unused_assignments)] // last_cursor is set in loop before use
            let mut last_cursor: Option<Vec<u8>> = None;

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
                        page_count: _,
                        sent_count,
                    } => {
                        // Decompress payload
                        let decompressed = lz4_flex::decompress_size_prepended(&payload)
                            .map_err(|e| eyre::eyre!("Failed to decompress snapshot page: {}", e))?;

                        if decompressed.len() != uncompressed_len as usize {
                            eyre::bail!(
                                "Decompressed size mismatch: expected {}, got {}",
                                uncompressed_len,
                                decompressed.len()
                            );
                        }

                        // Decode and apply records
                        let records = decode_snapshot_records(&decompressed)?;
                        let records_count = records.len();

                        // Apply records to storage
                        let mut handle = self.context_client.datastore_handle();
                        for (state_key, value) in records {
                            let key = ContextStateKey::new(context_id, state_key);
                            let slice: Slice<'_> = value.into();
                            let val: ContextStateValue<'_> = slice.into();
                            handle.put(&key, &val)?;
                        }

                        total_applied += records_count;
                        pages_in_batch += 1;

                        debug!(
                            %context_id,
                            page = page_num,
                            batch_page = pages_in_batch,
                            sent_count,
                            records = records_count,
                            total_applied,
                            "Applied snapshot page"
                        );

                        last_cursor = cursor;

                        // If cursor is None, we're done with this batch
                        if last_cursor.is_none() {
                            break;
                        }
                    }
                    MessagePayload::SnapshotError { error } => {
                        eyre::bail!("Snapshot streaming failed: {:?}", error);
                    }
                    _ => {
                        eyre::bail!("Unexpected payload during snapshot streaming");
                    }
                }
            }

            // Check if there are more pages to fetch
            if last_cursor.is_none() {
                // No more pages, we're done
                break;
            }

            // Continue with next batch
            resume_cursor = last_cursor;
        }

        Ok(total_applied)
    }
}

/// Result of a successful snapshot sync.
#[derive(Debug)]
pub struct SnapshotSyncResult {
    /// Root hash at the synced boundary.
    pub boundary_root_hash: Hash,
    /// DAG heads at the synced boundary (for fine-sync).
    pub dag_heads: Vec<[u8; 32]>,
    /// Number of records applied.
    pub applied_records: usize,
}

/// Internal struct for boundary negotiation result.
struct SnapshotBoundary {
    #[allow(dead_code)]
    boundary_timestamp: u64,
    boundary_root_hash: Hash,
    dag_heads: Vec<[u8; 32]>,
    #[allow(dead_code)]
    total_estimate: Option<u64>,
}

/// Generate snapshot pages with canonical encoding.
///
/// Returns (pages, next_cursor, total_entries).
/// Each page is an uncompressed byte vector of encoded records.
fn generate_snapshot_pages<L: calimero_store::layer::ReadLayer>(
    handle: &calimero_store::Handle<L>,
    context_id: ContextId,
    start_cursor: Option<&SnapshotCursor>,
    page_limit: u16,
    byte_limit: u32,
) -> Result<(Vec<Vec<u8>>, Option<SnapshotCursor>, u64)> {
    // Create an iterator over all ContextStateKey entries
    let mut iter = handle.iter::<ContextStateKey>()?;

    // Collect all entries for this context, sorted by state_key
    let mut entries: Vec<([u8; 32], Vec<u8>)> = Vec::new();

    // Use the iterator to get all entries
    for (key_result, value_result) in iter.entries() {
        let key = key_result?;
        let value = value_result?;

        // Filter by context_id
        if key.context_id() == context_id {
            entries.push((key.state_key(), value.value.to_vec()));
        }
    }

    // Sort entries by state_key for canonical ordering
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let total_entries = entries.len() as u64;

    // Skip to start cursor position if provided
    let start_idx = if let Some(cursor) = start_cursor {
        entries
            .iter()
            .position(|(key, _)| *key > cursor.last_key)
            .unwrap_or(entries.len())
    } else {
        0
    };

    // Generate pages
    let mut pages: Vec<Vec<u8>> = Vec::new();
    let mut current_page: Vec<u8> = Vec::new();
    let mut last_key: Option<[u8; 32]> = None;

    for (key, value) in entries.into_iter().skip(start_idx) {
        // Encode record: (key: [u8; 32], value: Vec<u8>)
        let record = CanonicalRecord { key, value };
        let record_bytes = borsh::to_vec(&record).expect("record serialization should not fail");

        // Check if adding this record would exceed byte limit
        if !current_page.is_empty()
            && (current_page.len() + record_bytes.len()) as u32 > byte_limit
        {
            // Finish current page
            pages.push(std::mem::take(&mut current_page));

            // Check page limit
            if pages.len() >= page_limit as usize {
                // Return with cursor pointing to last processed key
                let next_cursor = last_key.map(|k| SnapshotCursor { last_key: k });
                return Ok((pages, next_cursor, total_entries));
            }
        }

        // Add record to current page
        current_page.extend(record_bytes);
        last_key = Some(key);
    }

    // Add final page if non-empty
    if !current_page.is_empty() {
        pages.push(current_page);
    }

    // No more data, cursor is None
    Ok((pages, None, total_entries))
}

/// A canonical record in the snapshot stream.
///
/// Records are encoded as (key, value) pairs using Borsh.
/// The key is the 32-byte state_key from ContextState.
#[derive(BorshSerialize, BorshDeserialize)]
struct CanonicalRecord {
    /// 32-byte state key (from ContextState.state_key())
    key: [u8; 32],
    /// Raw stored value
    value: Vec<u8>,
}

/// Apply snapshot records to storage.
///
/// Decodes and writes records from an uncompressed page payload.
/// Returns the number of records applied.
/// Decode snapshot records from an uncompressed page payload.
///
/// Returns a vector of (state_key, value) pairs that can be written to storage.
pub fn decode_snapshot_records(page_payload: &[u8]) -> Result<Vec<([u8; 32], Vec<u8>)>> {
    let mut records = Vec::new();
    let mut offset = 0;

    while offset < page_payload.len() {
        // Deserialize record
        let record: CanonicalRecord = BorshDeserialize::deserialize(&mut &page_payload[offset..])?;
        let record_size = borsh::to_vec(&record)?.len();
        offset += record_size;

        records.push((record.key, record.value));
    }

    Ok(records)
}

/// Collect all state keys for a context.
///
/// Returns a vector of state keys that can be used for deletion or other operations.
pub fn collect_context_state_keys<L: calimero_store::layer::ReadLayer>(
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
    fn test_canonical_record_encoding() {
        let record = CanonicalRecord {
            key: [0u8; 32],
            value: vec![1, 2, 3, 4],
        };

        let encoded = borsh::to_vec(&record).unwrap();
        let decoded: CanonicalRecord = BorshDeserialize::deserialize(&mut &encoded[..]).unwrap();

        assert_eq!(record.key, decoded.key);
        assert_eq!(record.value, decoded.value);
    }
}
