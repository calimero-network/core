use std::sync::Arc;

use calimero_blobstore::{Blob, Size};
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key;
use calimero_store::layer::LayerExt;
use eyre::bail;
use futures_util::{AsyncRead, StreamExt};
use libp2p::PeerId;

use super::NodeClient;
use crate::messages::get_blob_bytes::GetBlobBytesRequest;
use crate::messages::NodeMessage::GetBlobBytes;

impl NodeClient {
    // todo! maybe this should be an actor method?
    // todo! so we can cache the blob in case it's
    // todo! to be immediately used? might require
    // todo! refactoring the blobstore API
    pub async fn add_blob<S: AsyncRead>(
        &self,
        stream: S,
        expected_size: Option<u64>,
        expected_hash: Option<&Hash>,
    ) -> eyre::Result<(BlobId, u64)> {
        let (blob_id, hash, size) = self
            .blobstore
            .put_sized(expected_size.map(Size::Exact), stream)
            .await?;

        if matches!(expected_hash, Some(expected_hash) if hash != *expected_hash) {
            bail!("fatal: blob hash mismatch");
        }

        if matches!(expected_size, Some(expected_size) if size != expected_size) {
            bail!("fatal: blob size mismatch");
        }

        Ok((blob_id, size))
    }

    /// Get blob from local storage or network if context_id is provided
    /// Returns a streaming Blob that can be used to read the data
    pub fn get_blob<'a>(
        &'a self,
        blob_id: &'a BlobId,
        context_id: Option<&'a ContextId>,
    ) -> impl std::future::Future<Output = eyre::Result<Option<Blob>>> + 'a {
        async move {
            // First try to get locally
            let Some(stream) = self.blobstore.get(*blob_id)? else {
                // If no context provided or blob not found locally, return None
                if context_id.is_none() {
                    return Ok(None);
                }

                // Try network discovery
                let context_id = context_id.unwrap();
                tracing::info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    "Blob not found locally, attempting network discovery"
                );

                const MAX_RETRIES: usize = 3;
                const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

                for attempt in 1..=MAX_RETRIES {
                    tracing::debug!(
                        blob_id = %blob_id,
                        context_id = %context_id,
                        attempt,
                        max_attempts = MAX_RETRIES,
                        "Attempting network discovery"
                    );

                    let peers = match self
                        .network_client
                        .query_blob(*blob_id, Some(*context_id))
                        .await
                    {
                        Ok(peers) => peers,
                        Err(e) => {
                            tracing::warn!(
                                blob_id = %blob_id,
                                context_id = %context_id,
                                attempt,
                                error = %e,
                                "Failed to query DHT for blob"
                            );
                            if attempt < MAX_RETRIES {
                                tokio::time::sleep(RETRY_DELAY).await;
                                continue;
                            }
                            return Err(e);
                        }
                    };

                    if peers.is_empty() {
                        tracing::info!(
                            blob_id = %blob_id,
                            context_id = %context_id,
                            attempt,
                            "No peers found with blob"
                        );
                        if attempt < MAX_RETRIES {
                            tokio::time::sleep(RETRY_DELAY).await;
                            continue;
                        }
                        return Ok(None);
                    }

                    tracing::info!(
                        blob_id = %blob_id,
                        context_id = %context_id,
                        peer_count = peers.len(),
                        attempt,
                        "Found {} peers with blob, attempting download", peers.len()
                    );

                    // Try to get the blob from each available peer
                    for (peer_index, peer_id) in peers.iter().enumerate() {
                        tracing::debug!(
                            peer_id = %peer_id,
                            peer_index = peer_index + 1,
                            total_peers = peers.len(),
                            attempt,
                            "Attempting to download blob from peer"
                        );

                        match self
                            .network_client
                            .request_blob(*blob_id, *context_id, *peer_id)
                            .await
                        {
                            Ok(Some(data)) => {
                                tracing::info!(
                                    blob_id = %blob_id,
                                    peer_id = %peer_id,
                                    size = data.len(),
                                    attempt,
                                    "Successfully downloaded blob from network"
                                );

                                // Store the blob locally for future use
                                let (blob_id_stored, _size) = self
                                    .add_blob(data.as_slice(), Some(data.len() as u64), None)
                                    .await?;

                                // Verify we stored the correct blob
                                if blob_id_stored != *blob_id {
                                    tracing::warn!(
                                        expected = %blob_id,
                                        actual = %blob_id_stored,
                                        "Downloaded blob ID mismatch"
                                    );
                                    continue;
                                }

                                // Return the newly stored blob as a stream
                                return Ok(self.blobstore.get(*blob_id)?);
                            }
                            Ok(None) => {
                                tracing::debug!(
                                    peer_id = %peer_id,
                                    attempt,
                                    "Peer doesn't have the blob"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    peer_id = %peer_id,
                                    error = %e,
                                    attempt,
                                    "Failed to download blob from peer"
                                );
                            }
                        }
                    }

                    // If we reach here, all peers failed for this attempt
                    if attempt < MAX_RETRIES {
                        tracing::info!(
                            blob_id = %blob_id,
                            context_id = %context_id,
                            attempt,
                            "All peers failed, retrying in {} seconds",
                            RETRY_DELAY.as_secs()
                        );
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }

                tracing::debug!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    max_attempts = MAX_RETRIES,
                    "Failed to download blob from any peer after all retry attempts"
                );
                return Ok(None);
            };

            Ok(Some(stream))
        }
    }

    /// Get blob bytes from local storage with actor-based caching
    /// Falls back to network download if context_id is provided and blob not found locally
    pub async fn get_blob_bytes(
        &self,
        blob_id: &BlobId,
        context_id: Option<&ContextId>,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        if **blob_id == [0; 32] {
            return Ok(None);
        }

        // First try to get from NodeManager's cache (for locally stored blobs)
        let request = GetBlobBytesRequest { blob_id: *blob_id };

        let (tx, rx) = tokio::sync::oneshot::channel();

        match self
            .node_manager
            .send(GetBlobBytes {
                request,
                outcome: tx,
            })
            .await
        {
            Ok(_) => {
                if let Ok(response) = rx.await {
                    if let Ok(response) = response {
                        if response.bytes.is_some() {
                            return Ok(response.bytes);
                        }
                    }
                }
            }
            Err(_) => {
                // NodeManager not available, fallback to direct access
            }
        }

        if let Some(context_id) = context_id {
            let Some(mut blob) = self.get_blob(blob_id, Some(context_id)).await? else {
                return Ok(None);
            };

            let mut data = Vec::new();
            while let Some(chunk) = blob.next().await {
                data.extend_from_slice(&chunk?);
            }

            Ok(Some(data.into()))
        } else {
            // No context_id provided and not in local cache
            Ok(None)
        }
    }

    /// Query the network for peers that have a specific blob
    pub async fn find_blob_providers(
        &self,
        blob_id: &BlobId,
        context_id: &ContextId,
    ) -> eyre::Result<Vec<PeerId>> {
        self.network_client
            .query_blob(*blob_id, Some(*context_id))
            .await
    }

    /// Announce a blob to the network for discovery
    pub async fn announce_blob_to_network(
        &self,
        blob_id: &BlobId,
        context_id: &ContextId,
        size: u64,
    ) -> eyre::Result<()> {
        self.network_client
            .announce_blob(*blob_id, *context_id, size)
            .await
    }

    pub fn has_blob(&self, blob_id: &BlobId) -> eyre::Result<bool> {
        self.blobstore.has(*blob_id)
    }

    /// List all root blobs
    ///
    /// Returns a list of all root blob IDs and their metadata. Root blobs are either:
    /// - Blobs that contain links to chunks (segmented large files)
    /// - Standalone blobs that aren't referenced as chunks by other blobs
    /// This excludes individual chunk blobs to provide a cleaner user experience.
    pub fn list_blobs(&self) -> eyre::Result<Vec<BlobInfo>> {
        let handle = self.datastore.clone().handle();

        let iter_result = handle.iter::<key::BlobMeta>();
        let mut iter = match iter_result {
            Ok(iter) => iter,
            Err(err) => {
                tracing::error!("Failed to create blob iterator: {:?}", err);
                bail!("Failed to iterate blob entries");
            }
        };

        let mut chunk_blob_ids = std::collections::HashSet::new();

        tracing::debug!("Starting first pass: collecting chunk blob IDs");
        for result in iter.entries() {
            match result {
                (Ok(_blob_key), Ok(blob_meta)) => {
                    // Only collect chunk IDs, not full blob info
                    for link in blob_meta.links.iter() {
                        let _ = chunk_blob_ids.insert(link.blob_id());
                    }
                }
                (Err(err), _) | (_, Err(err)) => {
                    tracing::error!(
                        "Failed to read blob entry during chunk collection: {:?}",
                        err
                    );
                    bail!("Failed to read blob entries");
                }
            }
        }

        let handle2 = self.datastore.clone().handle();
        let iter_result2 = handle2.iter::<key::BlobMeta>();
        let mut iter2 = match iter_result2 {
            Ok(iter) => iter,
            Err(err) => {
                tracing::error!("Failed to create second blob iterator: {:?}", err);
                bail!("Failed to iterate blob entries");
            }
        };

        let mut root_blobs = Vec::new();

        tracing::debug!(
            "Starting second pass: collecting root blobs (filtering {} chunks)",
            chunk_blob_ids.len()
        );
        for result in iter2.entries() {
            match result {
                (Ok(blob_key), Ok(blob_meta)) => {
                    let blob_id = blob_key.blob_id();

                    // Only include if it's not a chunk blob
                    if !chunk_blob_ids.contains(&blob_id) {
                        root_blobs.push(BlobInfo {
                            blob_id,
                            size: blob_meta.size,
                        });
                    }
                }
                (Err(err), _) | (_, Err(err)) => {
                    tracing::error!(
                        "Failed to read blob entry during root collection: {:?}",
                        err
                    );
                    bail!("Failed to read blob entries");
                }
            }
        }

        tracing::debug!(
            "Listing complete: found {} chunks, returning {} root/standalone blobs",
            chunk_blob_ids.len(),
            root_blobs.len()
        );

        Ok(root_blobs)
    }

    /// Delete a blob by its ID
    ///
    /// Removes blob metadata from database and deletes the actual blob files.
    /// This includes all associated chunk files for large blobs.
    pub async fn delete_blob(&self, blob_id: BlobId) -> eyre::Result<bool> {
        let mut handle = self.datastore.clone().handle();
        let blob_key = key::BlobMeta::new(blob_id);

        let blob_meta = match handle.get(&blob_key) {
            Ok(Some(meta)) => meta,
            Ok(None) => {
                bail!("Blob not found");
            }
            Err(err) => {
                tracing::error!("Failed to get blob metadata {}: {:?}", blob_id, err);
                bail!("Failed to access blob metadata: {}", err);
            }
        };

        tracing::info!(
            "Starting deletion for blob {} with {} linked chunks",
            blob_id,
            blob_meta.links.len()
        );

        let mut blobs_to_delete = vec![blob_id];
        let mut deleted_metadata_count = 0;
        let mut deleted_files_count = 0;

        blobs_to_delete.extend(blob_meta.links.iter().map(|link| link.blob_id()));

        // Delete blob files first
        for current_blob_id in &blobs_to_delete {
            match self.blobstore.delete(*current_blob_id).await {
                Ok(true) => {
                    deleted_files_count += 1;
                    tracing::debug!("Successfully deleted blob file {}", current_blob_id);
                }
                Ok(false) => {
                    tracing::debug!("Blob file {} was already missing", current_blob_id);
                }
                Err(err) => {
                    tracing::warn!("Failed to delete blob file {}: {}", current_blob_id, err);
                    // Continue with metadata deletion even if file deletion fails
                }
            }
        }

        // Delete metadata
        for current_blob_id in blobs_to_delete {
            let current_key = key::BlobMeta::new(current_blob_id);

            match handle.delete(&current_key) {
                Ok(()) => {
                    deleted_metadata_count += 1;
                    tracing::debug!("Successfully deleted metadata for blob {}", current_blob_id);
                }
                Err(err) => {
                    tracing::warn!(
                        "Failed to delete metadata for blob {}: {}",
                        current_blob_id,
                        err
                    );
                }
            }
        }

        if deleted_metadata_count > 0 {
            tracing::info!(
                "Successfully deleted {} blob metadata entries and {} blob files",
                deleted_metadata_count,
                deleted_files_count
            );
            Ok(true)
        } else {
            bail!("Failed to delete any blob metadata");
        }
    }

    /// Get blob metadata
    ///
    /// Returns blob metadata including size, hash, and detected MIME type.
    /// This is efficient for checking blob existence and getting metadata info.
    pub async fn get_blob_info(&self, blob_id: BlobId) -> eyre::Result<Option<BlobMetadata>> {
        let handle = self.datastore.clone().handle();
        let blob_key = key::BlobMeta::new(blob_id);

        match handle.get(&blob_key) {
            Ok(Some(blob_meta)) => {
                let mime_type = self
                    .detect_blob_mime_type(blob_id)
                    .await
                    .unwrap_or_else(|| "application/octet-stream".to_owned());

                Ok(Some(BlobMetadata {
                    blob_id,
                    size: blob_meta.size,
                    hash: blob_meta.hash,
                    mime_type,
                }))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                tracing::error!("Failed to get blob metadata: {:?}", err);
                bail!("Failed to retrieve blob metadata: {}", err);
            }
        }
    }

    /// Detect MIME type by reading the first few bytes of a blob
    pub async fn detect_blob_mime_type(&self, blob_id: BlobId) -> Option<String> {
        match self.get_blob(&blob_id, None).await {
            Ok(Some(mut blob_stream)) => {
                if let Some(Ok(first_chunk)) = blob_stream.next().await {
                    let bytes = first_chunk.as_ref();
                    let sample_size = std::cmp::min(bytes.len(), 512);
                    return Some(detect_mime_from_bytes(&bytes[..sample_size]).to_owned());
                }
            }
            Ok(None) => {
                tracing::warn!("Blob {} not found for MIME detection", blob_id);
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to read blob {} for MIME detection: {:?}",
                    blob_id,
                    err
                );
            }
        }

        None
    }
}

/// Detect MIME type from file bytes using the infer crate
fn detect_mime_from_bytes(bytes: &[u8]) -> &'static str {
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type();
    }

    "application/octet-stream"
}
