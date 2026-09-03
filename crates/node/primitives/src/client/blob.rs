use std::sync::Arc;

use calimero_blobstore::{Blob, BlobManager as BlobStore, Size};
use calimero_context_config::MAX_NAMESPACE_DEPTH;
use calimero_network_primitives::blob_types::{BlobAuth, BlobAuthPayload};
use calimero_primitives::{
    blobs::{BlobId, BlobInfo, BlobMetadata},
    common::DIGEST_SIZE,
    content_hash::ContentHash,
    context::ContextId,
    identity::{PrivateKey, PublicKey},
};
use calimero_store::key;
use calimero_store::layer::LayerExt;
use calimero_store::namespace_signer::resolve_owned_namespace_signer;
use eyre::bail;
use futures_util::future::join_all;
use futures_util::{AsyncRead, StreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, trace};

use super::NodeClient;
use crate::messages::get_blob_bytes::GetBlobBytesRequest;
use crate::messages::NodeMessage::GetBlobBytes;

/// Facade for blob storage used by [`NodeClient`].
///
/// Wraps [`calimero_blobstore::BlobManager`] (the store + filesystem implementation).
#[derive(Clone, Debug)]
pub struct BlobManager {
    pub(crate) blobstore: BlobStore,
}

impl BlobManager {
    #[must_use]
    pub fn new(blobstore: BlobStore) -> Self {
        Self { blobstore }
    }

    pub async fn add_blob<S: AsyncRead>(
        &self,
        stream: S,
        expected_size: Option<u64>,
        expected_content_hash: Option<&ContentHash>,
    ) -> eyre::Result<(BlobId, u64)> {
        debug!(
            expected_size,
            has_expected_hash = expected_content_hash.is_some(),
            "add_blob invoked"
        );

        let (blob_id, hash, size) = match self
            .blobstore
            .put_sized(expected_size.map(Size::Exact), stream)
            .await
        {
            Ok(result) => {
                trace!(
                    blob_id = %result.0,
                    stored_size = result.2,
                    hash = ?result.1,
                    "blobstore.put_sized completed"
                );
                result
            }
            Err(err) => {
                error!(error = ?err, "blobstore.put_sized failed");
                return Err(err);
            }
        };

        if matches!(expected_content_hash, Some(expected_content_hash) if hash != *expected_content_hash)
        {
            bail!("fatal: blob hash mismatch");
        }

        if matches!(expected_size, Some(expected_size) if size != expected_size) {
            bail!("fatal: blob size mismatch");
        }

        debug!(
            %blob_id,
            stored_size = size,
            "add_blob completed successfully"
        );

        Ok((blob_id, size))
    }

    pub fn has_blob(&self, blob_id: &BlobId) -> eyre::Result<bool> {
        self.blobstore.has(*blob_id)
    }

    pub fn get_blob_stream(&self, blob_id: BlobId) -> eyre::Result<Option<Blob>> {
        self.blobstore.get(blob_id)
    }

    /// Release one reference to `blob_id`, deleting its files and metadata (and
    /// those of its chunks) only once the last reference is gone. Content is
    /// deduplicated by hash, so several owners can share the same blob; the
    /// refcount-aware, tree-aware deletion lives in
    /// [`calimero_blobstore::BlobManager::delete`].
    pub async fn delete_blob(&self, blob_id: BlobId) -> eyre::Result<bool> {
        self.blobstore.delete(blob_id).await
    }
}

/// How many peers are probed at once when looking for a blob's holder.
///
/// Bounded on purpose: the candidate list is the whole subscriber set of a
/// context topic (up to `MAX_ESTABLISHED_TOTAL` peers), and probing all of it
/// on every local miss is the fan-out this discovery path exists to avoid. At a
/// realistic holder fraction of 0.2-0.3, eight probes answer 83-94% of misses,
/// and a miss only widens the search by one more batch.
const PROBE_BATCH: usize = 8;

/// How many times the whole discovery sweep is retried before giving up.
///
/// A sweep can come back empty because the answer is genuinely "nobody", or
/// because this node has only just joined and the context topic's subscriber
/// set is still filling. Those are indistinguishable from here, and widening
/// the batch cannot tell them apart — widening an empty candidate list yields
/// nothing. So the sweep is retried, re-resolving the candidates each time.
const MAX_DISCOVERY_ATTEMPTS: usize = 6;

/// First retry gap; each attempt doubles it up to [`MAX_RETRY_DELAY`].
const INITIAL_RETRY_DELAY: core::time::Duration = core::time::Duration::from_millis(100);

/// Ceiling on the retry gap.
const MAX_RETRY_DELAY: core::time::Duration = core::time::Duration::from_secs(2);

/// 100ms, doubling per attempt, capped at [`MAX_RETRY_DELAY`]. The `.min(31)`
/// bounds the shift so it stays well-defined if [`MAX_DISCOVERY_ATTEMPTS`] is
/// ever raised past 32.
fn discovery_backoff(attempt: usize) -> core::time::Duration {
    // `saturating_sub(1)` so the shift is underflow-safe even if the loop
    // bounds ever start at 0 (today it's 1..=MAX).
    INITIAL_RETRY_DELAY
        .saturating_mul(1_u32 << (attempt.saturating_sub(1) as u32).min(31))
        .min(MAX_RETRY_DELAY)
}

/// Index of the first peer in `candidates` that answers "yes, I hold it".
///
/// Probes go out `PROBE_BATCH` at a time, concurrently within a batch (so one
/// slow peer costs the batch a single probe timeout, not one per peer) and
/// strictly sequentially between batches: a batch containing a holder ends the
/// search, and no peer after it is ever probed. Within a batch, the
/// earliest-listed holder wins, which keeps the choice deterministic.
///
/// `probe` must answer `false` for a peer it cannot reach — a search with other
/// candidates left should not be aborted by one unreachable peer.
async fn find_blob_holder<P, F>(candidates: &[PeerId], probe: P) -> Option<usize>
where
    P: Fn(PeerId) -> F,
    F: core::future::Future<Output = bool>,
{
    for (batch_index, batch) in candidates.chunks(PROBE_BATCH).enumerate() {
        let answers = join_all(batch.iter().map(|peer_id| {
            let peer_id = *peer_id;
            let probe = &probe;
            async move { probe(peer_id).await }
        }))
        .await;

        if let Some(offset) = answers.into_iter().position(|holds| holds) {
            return Some(batch_index * PROBE_BATCH + offset);
        }
    }

    None
}

/// Fetch the blob from the first holder in `candidates` that actually delivers.
///
/// A failed fetch is NOT evidence that the blob is gone: the peer may have
/// dropped the connection, timed out, or served bytes whose recomputed id does
/// not match. Any of those says something about that peer, not about the blob,
/// so the search resumes at the candidate after it rather than reporting
/// absence. `fetch` returns `None` for exactly that case, and `Some(value)` —
/// including a failure value the caller wants propagated — to end the search.
///
/// Resuming re-probes the rest of the failed peer's batch, which is a handful
/// of wasted header exchanges in an already-degraded case; the short-circuit
/// property is untouched, since a search whose first winner delivers still
/// probes exactly one batch.
async fn fetch_from_first_holder<T, P, PF, D, DF>(
    candidates: &[PeerId],
    probe: P,
    fetch: D,
) -> Option<T>
where
    P: Fn(PeerId) -> PF,
    PF: core::future::Future<Output = bool>,
    D: Fn(PeerId) -> DF,
    DF: core::future::Future<Output = Option<T>>,
{
    let mut searched = 0;

    while searched < candidates.len() {
        let found = find_blob_holder(&candidates[searched..], &probe).await?;
        let peer_id = candidates[searched + found];

        if let Some(value) = fetch(peer_id).await {
            return Some(value);
        }

        searched += found + 1;
    }

    None
}

/// Discover a holder among the context's peers and fetch the blob from it,
/// retrying the whole sweep while the candidate set may still be filling.
///
/// `resolve_candidates` is called afresh on every attempt: the reason to retry
/// at all is that the subscriber set grows, so re-using a stale (often empty)
/// list would make the retries pointless.
async fn discover_and_fetch_blob<T, C, CF, P, PF, D, DF>(
    resolve_candidates: C,
    probe: P,
    fetch: D,
) -> Option<T>
where
    C: Fn() -> CF,
    CF: core::future::Future<Output = Vec<PeerId>>,
    P: Fn(PeerId) -> PF,
    PF: core::future::Future<Output = bool>,
    D: Fn(PeerId) -> DF,
    DF: core::future::Future<Output = Option<T>>,
{
    for attempt in 1..=MAX_DISCOVERY_ATTEMPTS {
        let candidates = resolve_candidates().await;

        if let Some(value) = fetch_from_first_holder(&candidates, &probe, &fetch).await {
            return Some(value);
        }

        if attempt < MAX_DISCOVERY_ATTEMPTS {
            tokio::time::sleep(discovery_backoff(attempt)).await;
        }
    }

    None
}

impl NodeClient {
    // todo! maybe this should be an actor method?
    // todo! so we can cache the blob in case it's
    // todo! to be immediately used? might require
    // todo! refactoring the blobstore API
    pub async fn add_blob<S: AsyncRead>(
        &self,
        stream: S,
        expected_size: Option<u64>,
        expected_content_hash: Option<&ContentHash>,
    ) -> eyre::Result<(BlobId, u64)> {
        self.blob_manager
            .add_blob(stream, expected_size, expected_content_hash)
            .await
    }

    /// Get blob from local storage or network if context_id is provided
    /// Returns a streaming Blob that can be used to read the data
    pub async fn get_blob<'a>(
        &'a self,
        blob_id: &'a BlobId,
        context_id: Option<&'a ContextId>,
    ) -> eyre::Result<Option<Blob>> {
        // First try to get locally
        let Some(stream) = self.blob_manager.get_blob_stream(*blob_id)? else {
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

            // Ask the context's peers who holds this blob, rather than
            // reading a single-valued DHT provider record: custody is a
            // property of a peer's own blob store, so the peer is the
            // authority on it and the answer cannot go stale.
            //
            // `subscribed_peers` is the full connected+subscribed set, not the
            // grafted mesh, so probing all of it on every miss is exactly the
            // fan-out this design exists to avoid. Probes therefore go out in
            // bounded batches, short-circuiting on the first holder, and the
            // whole sweep is retried while the subscriber set may still be
            // filling underneath us.
            //
            // Generate authorization for the blob once. The same signed proof
            // authorizes both the probes and the fetch that follows them — a
            // peer answers `found: false` to an unauthorized probe, so probing
            // without it would find only public blobs.
            let auth = self.create_blob_auth_for_context(context_id, blob_id)?;

            let fetched = discover_and_fetch_blob(
                || async {
                    self.network_client
                        .subscribed_peers(TopicHash::from_raw(*context_id))
                        .await
                },
                |peer_id| async move {
                    self.network_client
                        .probe_blob(*blob_id, *context_id, peer_id, auth)
                        .await
                        .unwrap_or(false)
                },
                |peer_id| async move {
                    tracing::info!(
                        blob_id = %blob_id,
                        context_id = %context_id,
                        peer_id = %peer_id,
                        "Found a peer holding the blob, downloading"
                    );

                    let data = match self
                        .network_client
                        .request_blob(*blob_id, *context_id, peer_id, auth)
                        .await
                    {
                        Ok(Some(data)) => data,
                        Ok(None) => {
                            // The peer said yes to the probe and no to the
                            // fetch: custody changed underneath us, or it
                            // declined. That is a fact about this peer, not
                            // about the blob — keep searching.
                            tracing::warn!(
                                blob_id = %blob_id,
                                peer_id = %peer_id,
                                "Peer answered the probe but did not serve the blob"
                            );
                            return None;
                        }
                        Err(e) => {
                            tracing::warn!(
                                blob_id = %blob_id,
                                peer_id = %peer_id,
                                error = %e,
                                "Failed to download blob from peer, trying the next holder"
                            );
                            return None;
                        }
                    };

                    tracing::info!(
                        blob_id = %blob_id,
                        peer_id = %peer_id,
                        size = data.len(),
                        "Successfully downloaded blob from network"
                    );

                    // Store the blob locally for future use. A failure here is
                    // local, so it will fail identically for every other
                    // holder: end the search and surface it.
                    let (blob_id_stored, _size) = match self
                        .add_blob(data.as_slice(), Some(data.len() as u64), None)
                        .await
                    {
                        Ok(stored) => stored,
                        Err(e) => return Some(Err(e)),
                    };

                    // Blobs are content-addressed, so a mismatch means this
                    // peer served different bytes than were asked for. Refuse
                    // them and ask the next holder.
                    if blob_id_stored != *blob_id {
                        tracing::warn!(
                            expected = %blob_id,
                            actual = %blob_id_stored,
                            peer_id = %peer_id,
                            "Downloaded blob ID mismatch, trying the next holder"
                        );
                        return None;
                    }

                    // Return the newly stored blob as a stream
                    Some(self.blob_manager.get_blob_stream(*blob_id))
                },
            )
            .await;

            let Some(result) = fetched else {
                tracing::info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    attempts = MAX_DISCOVERY_ATTEMPTS,
                    "No context peer served this blob"
                );
                return Ok(None);
            };

            return result;
        };

        Ok(Some(stream))
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

        let blob_id = *blob_id;

        // Try NodeManager's cache first (checks cache, then blobstore if not cached, and updates cache)
        // This ensures proper caching behavior and access tracking
        let request = GetBlobBytesRequest { blob_id };
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Single bound for the whole NodeManager round-trip, applied to BOTH
        // legs (enqueue and async reply). Keeping them equal is the point: the
        // previous 10ms enqueue / 100ms reply pair raced the actor under load.
        // On the enqueue leg, `send()` resolves once the message is dequeued —
        // fast unless the actor is truly gone; a dead/absent actor's `send()`
        // errors quickly, so the bound only really fires when the mailbox is
        // deeply backed up. On the reply leg, the handler answers asynchronously
        // via `tx` AFTER reading (and caching) the blob, which can legitimately
        // exceed 100ms for a large blob; a too-short reply bound abandoned the
        // actor path and re-read the same blob from disk here while the actor
        // was still writing the result into the now-dropped `tx` — a wasted
        // second disk read on every contended call. One generous, shared bound
        // eliminates both races; if the actor genuinely stalls past it, `rx`
        // (or `send`) still lets us fall through to the direct read below.
        const NODE_MANAGER_BLOB_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(1);

        let send_result = tokio::time::timeout(
            NODE_MANAGER_BLOB_TIMEOUT,
            self.node_manager.send(GetBlobBytes {
                request,
                outcome: tx,
            }),
        )
        .await;

        if let Ok(Ok(())) = send_result {
            // Node manager accepted the request, wait for response with timeout
            match tokio::time::timeout(NODE_MANAGER_BLOB_TIMEOUT, rx).await {
                Ok(Ok(Ok(response))) if response.bytes.is_some() => {
                    return Ok(response.bytes);
                }
                Ok(Ok(Ok(_))) => {
                    // NodeManager returned None (blob not found), fall through to direct blobstore
                }
                _ => {
                    // Node manager didn't respond in time, fall through to direct blobstore
                }
            }
        }

        // Fallback to direct blobstore access if NodeManager is unavailable or blob not in cache
        // This ensures we can still retrieve blobs even if NodeManager is down (e.g., in tests)
        if let Some(mut stream) = self.blob_manager.get_blob_stream(blob_id)? {
            let mut data = Vec::new();
            while let Some(chunk) = stream.next().await {
                data.extend_from_slice(&chunk?);
            }
            return Ok(Some(data.into()));
        }

        // If not found locally and context_id provided, try network discovery
        if let Some(context_id) = context_id {
            let Some(mut blob) = self.get_blob(&blob_id, Some(context_id)).await? else {
                return Ok(None);
            };

            let mut data = Vec::new();
            while let Some(chunk) = blob.next().await {
                data.extend_from_slice(&chunk?);
            }

            Ok(Some(data.into()))
        } else {
            // No context_id provided and blob not found locally
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
        self.blob_manager.has_blob(blob_id)
    }

    /// List all root blobs
    ///
    /// Returns a list of all root blob IDs and their metadata. Root blobs are either:
    /// - Blobs that contain links to chunks (segmented large files)
    /// - Standalone blobs that aren't referenced as chunks by other blobs
    ///
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
                    for link in &blob_meta.links {
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

    /// Release this owner's reference to a blob by its ID.
    ///
    /// Blobs are content-addressed and deduplicated: the same bytes can be
    /// referenced by several contexts, and a chunk can be shared by several root
    /// blobs. So this does not delete eagerly — it decrements the blob's (and,
    /// for a root blob, each chunk's) reference count and removes the files and
    /// metadata only once nothing references them. Deleting eagerly here would
    /// corrupt other owners' blobs that share the same content.
    ///
    /// Returns `Ok(true)` when the blob existed and its reference was released;
    /// errors with "Blob not found" when it was already absent.
    pub async fn delete_blob(&self, blob_id: BlobId) -> eyre::Result<bool> {
        match self.blob_manager.delete_blob(blob_id).await {
            Ok(true) => {
                tracing::info!(%blob_id, "released blob reference");
                Ok(true)
            }
            Ok(false) => bail!("Blob not found"),
            Err(err) => {
                tracing::error!("Failed to delete blob {}: {:?}", blob_id, err);
                bail!("Failed to delete blob: {}", err);
            }
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
                    hash: blob_meta.content_hash.into(),
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
                    let sample_size = core::cmp::min(bytes.len(), 512);
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

    /// Helper to find an identity in the datastore for which the node possesses the private key.
    ///
    /// Two shapes of ownership must both resolve:
    ///  * **Stored key** — a standalone / `new_identity` context keeps its own
    ///    `private_key` on the [`key::ContextIdentity`] row.
    ///  * **Keyless marker** — in a namespace-backed context (every context
    ///    created under the group model) the member row carries no key; the node
    ///    signs with its single namespace identity, resolved live via
    ///    [`resolve_owned_namespace_signer`].
    ///
    /// Handling only the first case made this return `None` for contexts the node
    /// is genuinely a member of, which skipped blob announce and left every
    /// outgoing blob request unsigned — so peers refused to serve blobs to a
    /// legitimate member. [`ContextClient::get_identity`] resolves the same two
    /// shapes; both now share one implementation.
    ///
    /// [`ContextClient::get_identity`]: https://docs.rs/calimero-context-client
    pub fn find_owned_identity(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<(PublicKey, PrivateKey)>> {
        let handle = self.datastore.clone().handle();
        let start_key = key::ContextIdentity::new(*context_id, [0u8; DIGEST_SIZE].into());
        let mut iter = handle.iter::<key::ContextIdentity>()?;
        let first = iter.seek(start_key).transpose();

        // The namespace signer, resolved at most once and only once a keyless
        // marker row is actually met. `PrivateKey` is deliberately not `Clone`, so
        // it is held by value and moved out on the matching return.
        let mut ns_signer: Option<(PublicKey, PrivateKey)> = None;
        let mut ns_resolved = false;

        for key in first.into_iter().chain(iter.keys()) {
            let key = key?;
            if key.context_id() != *context_id {
                break;
            }

            if let Some(val) = handle.get(&key)? {
                if let Some(pk_bytes) = val.private_key {
                    return Ok(Some((key.public_key(), PrivateKey::from(pk_bytes))));
                }

                // Keyless marker: ours only if it names our namespace identity.
                if !ns_resolved {
                    ns_signer = resolve_owned_namespace_signer(
                        &self.datastore,
                        context_id,
                        MAX_NAMESPACE_DEPTH,
                    )?
                    .map(|(pk, sk)| (PublicKey::from(pk), PrivateKey::from(sk)));
                    ns_resolved = true;
                }
                if ns_signer
                    .as_ref()
                    .is_some_and(|(ns_pk, _)| *ns_pk == key.public_key())
                {
                    return Ok(ns_signer);
                }
            }
        }
        Ok(None)
    }

    /// Generates the `BlobAuth` authentication structure by creating a payload envelope and signing it.
    ///
    /// # Arguments
    /// * `blob_id` - The ID of the blob being requested.
    /// * `context_id` - The context context the blob belongs to.
    /// * `public_key` - The public key of the requester that is a member of the context.
    /// * `private_key` - The private key used to sign the request.
    pub fn create_blob_auth(
        &self,
        blob_id: &BlobId,
        context_id: &ContextId,
        public_key: PublicKey,
        private_key: &PrivateKey,
    ) -> eyre::Result<BlobAuth> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        // Construct the Envelope Payload
        let payload = BlobAuthPayload {
            blob_id: *blob_id.digest(),
            context_id: *context_id.digest(),
            timestamp,
        };

        // Serialize the envelope using Borsh
        let message = borsh::to_vec(&payload)?;

        // Sign the serialized envelope
        let signature = private_key
            .sign(&message)
            .map_err(|e| eyre::eyre!("Signing failed: {}", e))?;

        Ok(BlobAuth {
            public_key,
            signature: signature.to_bytes(),
            timestamp,
        })
    }

    /// A helper function that finds identity from store and creates blob authentication struct.
    ///
    /// Attempts to find a local identity for the context. If found, generates a signature.
    /// If not found, returns `None` (which implies a public access request).
    /// # Returns
    /// * `Ok(Some(blob_auth))` - if the local identity was found and blob authentication struct
    ///   was successfully created.
    /// * `Ok(None)` - if the node doesn't own any identity for the given context.
    /// * `Err` - if some internal error occured (e.g. DB error, serialization, etc).
    pub fn create_blob_auth_for_context(
        &self,
        context_id: &ContextId,
        blob_id: &BlobId,
    ) -> eyre::Result<Option<BlobAuth>> {
        if let Some((public_key, private_key)) = self.find_owned_identity(context_id)? {
            let auth = self.create_blob_auth(blob_id, context_id, public_key, &private_key)?;
            Ok(Some(auth))
        } else {
            Ok(None)
        }
    }
}

/// Detect MIME type from file bytes using the infer crate
fn detect_mime_from_bytes(bytes: &[u8]) -> &'static str {
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type();
    }

    "application/octet-stream"
}

#[cfg(test)]
mod blob_discovery_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::{
        discover_and_fetch_blob, fetch_from_first_holder, find_blob_holder, PeerId,
        MAX_DISCOVERY_ATTEMPTS, PROBE_BATCH,
    };

    /// Records who was probed, and how many probes were ever in flight at once.
    #[derive(Default)]
    struct ProbeRecorder {
        probed: Mutex<Vec<PeerId>>,
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    impl ProbeRecorder {
        /// Run one probe: enter, yield enough times that any concurrent sibling
        /// gets scheduled (so `peak_in_flight` reflects real overlap rather
        /// than luck), then leave with `answer`.
        async fn probe(&self, peer_id: PeerId, answer: bool) -> bool {
            self.probed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(peer_id);

            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let _ignored = self.peak_in_flight.fetch_max(now, Ordering::SeqCst);

            for _ in 0..(PROBE_BATCH * 2) {
                tokio::task::yield_now().await;
            }

            let _ignored = self.in_flight.fetch_sub(1, Ordering::SeqCst);
            answer
        }

        fn probed(&self) -> Vec<PeerId> {
            self.probed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    fn peers(count: usize) -> Vec<PeerId> {
        (0..count).map(|_| PeerId::random()).collect()
    }

    #[tokio::test]
    async fn never_exceeds_the_batch_bound() {
        // Far more candidates than one batch: the bound is what stops this from
        // becoming a broadcast to every subscriber of the context topic.
        let candidates = peers(PROBE_BATCH * 4 + 3);
        let recorder = ProbeRecorder::default();

        let holder = find_blob_holder(&candidates, |peer_id| recorder.probe(peer_id, false)).await;

        assert_eq!(holder, None);
        // Nobody holds it, so every candidate must have been asked...
        assert_eq!(recorder.probed().len(), candidates.len());
        // ...but never more than a batch at a time.
        assert_eq!(
            recorder.peak_in_flight.load(Ordering::SeqCst),
            PROBE_BATCH,
            "probes must run a full batch wide, and no wider"
        );
    }

    #[tokio::test]
    async fn stops_at_the_first_holder_without_probing_the_rest() {
        let candidates = peers(PROBE_BATCH * 3);
        let holder_index = 2;
        let recorder = ProbeRecorder::default();

        let holder = find_blob_holder(&candidates, |peer_id| {
            let answer = peer_id == candidates[holder_index];
            recorder.probe(peer_id, answer)
        })
        .await;

        assert_eq!(holder, Some(holder_index));
        // The holder is in the first batch, so the search ends there: the
        // batch it sits in is probed in full (those probes are concurrent and
        // already in flight), and not one peer beyond it.
        assert_eq!(recorder.probed(), candidates[..PROBE_BATCH].to_vec());
    }

    #[tokio::test]
    async fn widens_past_a_missing_batch() {
        let candidates = peers(PROBE_BATCH * 3);
        let holder_index = PROBE_BATCH + 1;
        let recorder = ProbeRecorder::default();

        let holder = find_blob_holder(&candidates, |peer_id| {
            let answer = peer_id == candidates[holder_index];
            recorder.probe(peer_id, answer)
        })
        .await;

        assert_eq!(holder, Some(holder_index));
        // Two batches: the miss widened the search exactly once, and the third
        // batch was never touched.
        assert_eq!(recorder.probed(), candidates[..PROBE_BATCH * 2].to_vec());
    }

    #[tokio::test]
    async fn earliest_holder_in_a_batch_wins() {
        // Determinism matters: concurrent probes complete in arbitrary order,
        // so the winner is chosen by candidate order, not by who answers first.
        let candidates = peers(PROBE_BATCH);
        let recorder = ProbeRecorder::default();

        let holder = find_blob_holder(&candidates, |peer_id| {
            let answer = peer_id == candidates[1] || peer_id == candidates[5];
            recorder.probe(peer_id, answer)
        })
        .await;

        assert_eq!(holder, Some(1));
    }

    #[tokio::test]
    async fn no_candidates_means_no_holder() {
        let recorder = ProbeRecorder::default();

        let holder = find_blob_holder(&[], |peer_id| recorder.probe(peer_id, true)).await;

        assert_eq!(holder, None);
        assert!(recorder.probed().is_empty());
    }

    #[tokio::test]
    async fn a_failed_fetch_falls_back_to_the_next_holder() {
        // Two holders in the same batch. The first one wins the probe and then
        // fails to deliver — which says something about that peer, not about
        // the blob — so the second must be asked.
        let candidates = peers(PROBE_BATCH * 2);
        let bad = candidates[1];
        let good = candidates[4];
        let recorder = ProbeRecorder::default();
        let fetched_from = Mutex::new(Vec::new());

        let result: Option<&str> = fetch_from_first_holder(
            &candidates,
            |peer_id| {
                let answer = peer_id == bad || peer_id == good;
                recorder.probe(peer_id, answer)
            },
            |peer_id| {
                let fetched_from = &fetched_from;
                async move {
                    fetched_from
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(peer_id);
                    (peer_id == good).then_some("bytes")
                }
            },
        )
        .await;

        assert_eq!(result, Some("bytes"));
        // Both were asked, in candidate order: the bad one first, then the
        // fallback. A single-holder implementation stops after the bad one.
        assert_eq!(
            *fetched_from
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![bad, good]
        );
        // The bound still holds while falling back.
        assert_eq!(recorder.peak_in_flight.load(Ordering::SeqCst), PROBE_BATCH);
    }

    #[tokio::test]
    async fn every_holder_failing_is_not_a_holder_found() {
        let candidates = peers(3);
        let recorder = ProbeRecorder::default();
        let fetches = AtomicUsize::new(0);

        let result: Option<&str> = fetch_from_first_holder(
            &candidates,
            |peer_id| recorder.probe(peer_id, true),
            |_peer_id| async {
                let _ignored = fetches.fetch_add(1, Ordering::SeqCst);
                None
            },
        )
        .await;

        assert_eq!(result, None);
        // Every candidate was fetched from before giving up: the fallback
        // walks the list rather than stopping at the first disappointment.
        assert_eq!(fetches.load(Ordering::SeqCst), candidates.len());
        // Resuming re-probes the remainder of the failed peer's batch, so all
        // three answer, then two, then one. That waste is the documented cost
        // of the fallback, and it is bounded by the batch, not by the list.
        assert_eq!(recorder.probed().len(), 3 + 2 + 1);
    }

    #[tokio::test]
    async fn a_successful_first_winner_still_probes_only_one_batch() {
        // The fallback must not have cost the short-circuit: the happy path is
        // still exactly one batch of probes and one fetch.
        let candidates = peers(PROBE_BATCH * 3);
        let recorder = ProbeRecorder::default();

        let result: Option<&str> = fetch_from_first_holder(
            &candidates,
            |peer_id| {
                let answer = peer_id == candidates[0];
                recorder.probe(peer_id, answer)
            },
            |_peer_id| async { Some("bytes") },
        )
        .await;

        assert_eq!(result, Some("bytes"));
        assert_eq!(recorder.probed(), candidates[..PROBE_BATCH].to_vec());
    }

    #[tokio::test(start_paused = true)]
    async fn retries_the_sweep_while_the_subscriber_set_fills() {
        // The regression this guards: a node that has just joined has no
        // subscribed peers yet. "No candidates" is not "nobody has it", and
        // widening an empty list yields nothing — only retrying can find the
        // holder that shows up on attempt 3.
        let late_holder = PeerId::random();
        let sweeps = AtomicUsize::new(0);

        let result: Option<&str> = discover_and_fetch_blob(
            || async {
                let sweep = sweeps.fetch_add(1, Ordering::SeqCst) + 1;
                if sweep < 3 {
                    Vec::new()
                } else {
                    vec![late_holder]
                }
            },
            |_peer_id| async { true },
            |_peer_id| async { Some("bytes") },
        )
        .await;

        assert_eq!(result, Some("bytes"));
        assert_eq!(sweeps.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_once_the_attempt_budget_is_spent() {
        let sweeps = AtomicUsize::new(0);

        let result: Option<&str> = discover_and_fetch_blob(
            || async {
                let _ignored = sweeps.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            },
            |_peer_id| async { true },
            |_peer_id| async { Some("bytes") },
        )
        .await;

        assert_eq!(result, None);
        assert_eq!(sweeps.load(Ordering::SeqCst), MAX_DISCOVERY_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn a_sweep_that_finds_a_holder_does_not_retry() {
        let holder = PeerId::random();
        let sweeps = AtomicUsize::new(0);

        let result: Option<&str> = discover_and_fetch_blob(
            || async {
                let _ignored = sweeps.fetch_add(1, Ordering::SeqCst);
                vec![holder]
            },
            |_peer_id| async { true },
            |_peer_id| async { Some("bytes") },
        )
        .await;

        assert_eq!(result, Some("bytes"));
        assert_eq!(sweeps.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        use super::{discovery_backoff, INITIAL_RETRY_DELAY, MAX_RETRY_DELAY};

        assert_eq!(discovery_backoff(1), INITIAL_RETRY_DELAY);
        assert_eq!(discovery_backoff(2), INITIAL_RETRY_DELAY * 2);
        assert_eq!(discovery_backoff(3), INITIAL_RETRY_DELAY * 4);
        assert_eq!(discovery_backoff(5), INITIAL_RETRY_DELAY * 16);
        // 100ms * 32 would be 3.2s, so attempt 6 is where the cap bites.
        assert_eq!(discovery_backoff(6), MAX_RETRY_DELAY);
        // Underflow- and overflow-safe at both ends of the range.
        assert_eq!(discovery_backoff(0), INITIAL_RETRY_DELAY);
        assert_eq!(discovery_backoff(usize::MAX), MAX_RETRY_DELAY);
    }
}
