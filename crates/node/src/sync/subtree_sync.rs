//! SubtreePrefetch sync protocol implementation.
//!
//! Two-phase protocol for efficient synchronization of deep trees with clustered changes:
//! - Phase 1: Discover remote prefixes via `SubtreePrefetchRequest` with empty roots
//! - Phase 2: Fetch entire divergent subtrees in one request
//!
//! Uses its own `SubtreePrefetchRequest` wire message for both phases, avoiding
//! any conflict with `TreeNodeRequest` used by HashComparison.
//!
//! Falls back to snapshot sync on any failure.

use std::collections::{HashMap, HashSet};

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::hash_comparison::{CrdtType, LeafMetadata, TreeLeafData};
use calimero_node_primitives::sync::subtree::{SubtreeData, SubtreePrefetchResponse};
use calimero_node_primitives::sync::{
    create_runtime_env, InitPayload, MessagePayload, StreamMessage, SyncProtocol,
};
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::env::with_runtime_env;
use calimero_storage::index::Index;
use calimero_storage::store::MainStorage;
use calimero_store::key::ContextState as ContextStateKey;
use eyre::WrapErr;
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

/// Number of leading key bytes used to define a subtree prefix.
///
/// Roots are constructed as `[prefix_byte, 0, …, 0]`, so all prefix-based
/// filtering (entity collection and stale-key detection) must compare exactly
/// this many bytes to avoid false negatives on keys with non-zero trailing bytes.
const SUBTREE_PREFIX_LEN: usize = 1;

// =============================================================================
// Responder Handlers
// =============================================================================

impl SyncManager {
    /// Handle a SubtreePrefetchRequest.
    ///
    /// Two modes depending on `subtree_roots`:
    /// - **Empty** (discovery / Phase 1): enumerate unique first-byte prefixes of
    ///   state keys and return one empty `SubtreeData` per prefix so the initiator
    ///   can identify divergent subtrees. Then read the follow-up Phase 2 request
    ///   from the same stream and handle it.
    /// - **Non-empty** (fetch / Phase 2): walk storage for entities matching the
    ///   requested subtree roots and send back a `SubtreePrefetchResponse`.
    pub(crate) async fn handle_subtree_prefetch_request(
        &self,
        context_id: ContextId,
        subtree_roots: Vec<[u8; 32]>,
        max_depth: Option<usize>,
        stream: &mut Stream,
        _nonce: calimero_crypto::Nonce,
    ) -> eyre::Result<()> {
        // Resolve identity for RuntimeEnv (needed to read CRDT metadata from Index)
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));
        let our_identity = match crate::utils::choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        {
            Some((identity, _)) => identity,
            None => {
                warn!(%context_id, "No owned identity, cannot respond to SubtreePrefetchRequest");
                return Ok(());
            }
        };

        let datastore = self.context_client.datastore_handle().into_inner();
        let runtime_env = create_runtime_env(&datastore, context_id, our_identity);

        if subtree_roots.is_empty() {
            // Discovery mode: enumerate local prefixes so the initiator can
            // compare them against its own state and send a targeted Phase 2.
            self.send_prefix_discovery(context_id, max_depth, stream)
                .await?;

            // Read the Phase 2 follow-up on the same stream
            let phase2 = match self.recv(stream, None).await {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    debug!(%context_id, "SubtreePrefetch discovery: stream closed after Phase 1");
                    return Ok(());
                }
                Err(e) => {
                    warn!(%context_id, error = %e, "SubtreePrefetch discovery: recv Phase 2 failed");
                    return Ok(());
                }
            };

            let (roots, depth) = match phase2 {
                StreamMessage::Init {
                    payload:
                        InitPayload::SubtreePrefetchRequest {
                            subtree_roots,
                            max_depth,
                            ..
                        },
                    ..
                } => (subtree_roots, max_depth),
                _ => {
                    debug!(%context_id, "SubtreePrefetch discovery: unexpected Phase 2 message type");
                    return Ok(());
                }
            };

            self.send_subtree_fetch(context_id, &roots, depth, stream, &runtime_env)
                .await
        } else {
            // Direct fetch (Phase 2 only)
            self.send_subtree_fetch(context_id, &subtree_roots, max_depth, stream, &runtime_env)
                .await
        }
    }

    /// Send prefix discovery response (Phase 1 responder).
    ///
    /// Enumerates unique first-byte prefixes in storage for the given context,
    /// computes a per-prefix XOR hash of entity keys, and returns one
    /// `SubtreeData` per prefix carrying that hash. The initiator uses the
    /// hashes to filter out shared prefixes whose data already matches,
    /// avoiding unnecessary Phase 2 fetches.
    async fn send_prefix_discovery(
        &self,
        context_id: ContextId,
        max_depth: Option<usize>,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        let depth =
            max_depth.unwrap_or(calimero_node_primitives::sync::subtree::DEFAULT_SUBTREE_MAX_DEPTH);

        let handle = self.context_client.datastore_handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        // Single scan: bucket entity keys+values by first-byte prefix and XOR-hash them.
        // Including values ensures that same-key-different-value divergence is detected.
        let mut prefix_hashes: HashMap<u8, [u8; 32]> = HashMap::new();
        for (key_result, value_result) in iter.entries() {
            let key = key_result?;
            let value = value_result?;
            if key.context_id() == context_id {
                let state_key = key.state_key();
                let hash = prefix_hashes.entry(state_key[0]).or_insert([0u8; 32]);
                xor_into_hash(hash, &state_key);
                xor_into_hash(hash, &value.value);
            }
        }

        let subtrees: Vec<SubtreeData> = prefix_hashes
            .into_iter()
            .map(|(prefix_byte, hash)| {
                let mut root_id = [0u8; 32];
                root_id[0] = prefix_byte;
                SubtreeData::new(root_id, hash, vec![], depth)
            })
            .collect();

        debug!(
            %context_id,
            prefix_count = subtrees.len(),
            "SubtreePrefetch discovery: sending prefix list with hashes"
        );

        let response = SubtreePrefetchResponse::new(subtrees, vec![]);
        let serialized =
            borsh::to_vec(&response).wrap_err("failed to serialize discovery response")?;

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SubtreePrefetchResponse {
                subtrees: serialized.into(),
                not_found_count: 0,
            },
            next_nonce: super::helpers::generate_nonce(),
        };
        super::stream::send(stream, &msg, None).await
    }

    /// Send subtree fetch response (Phase 2 responder).
    ///
    /// Collects entities for each requested subtree root and sends back a
    /// `SubtreePrefetchResponse`.
    async fn send_subtree_fetch(
        &self,
        context_id: ContextId,
        subtree_roots: &[[u8; 32]],
        max_depth: Option<usize>,
        stream: &mut Stream,
        runtime_env: &calimero_storage::env::RuntimeEnv,
    ) -> eyre::Result<()> {
        let depth =
            max_depth.unwrap_or(calimero_node_primitives::sync::subtree::DEFAULT_SUBTREE_MAX_DEPTH);

        info!(
            %context_id,
            subtree_count = subtree_roots.len(),
            depth,
            "SubtreePrefetch fetch: collecting subtree data"
        );

        let mut subtrees = Vec::new();
        let mut not_found = Vec::new();

        for root in subtree_roots {
            match self.collect_subtree_entities(context_id, root, depth, runtime_env) {
                Ok(subtree_data) => {
                    if subtree_data.is_empty() {
                        not_found.push(*root);
                    } else {
                        subtrees.push(subtree_data);
                    }
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        root = ?root,
                        error = %e,
                        "Failed to collect subtree entities"
                    );
                    not_found.push(*root);
                }
            }
        }

        let response = SubtreePrefetchResponse::new(subtrees, not_found.clone());
        let serialized =
            borsh::to_vec(&response).wrap_err("failed to serialize SubtreePrefetchResponse")?;

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::SubtreePrefetchResponse {
                subtrees: serialized.into(),
                not_found_count: not_found.len() as u32,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Collect all entities within a subtree identified by a root prefix.
    ///
    /// Iterates storage, filters by context_id and first-byte prefix match,
    /// and builds `TreeLeafData` for each matching entity with real CRDT
    /// metadata from the Index (Invariant I10: persist crdt_type).
    ///
    /// Note: roots are single-byte prefixes (`[prefix, 0, …, 0]`), so we
    /// always compare only the first byte. The `depth` parameter is stored
    /// in `SubtreeData` as truncation metadata but does not affect filtering.
    fn collect_subtree_entities(
        &self,
        context_id: ContextId,
        subtree_root: &[u8; 32],
        depth: usize,
        runtime_env: &calimero_storage::env::RuntimeEnv,
    ) -> eyre::Result<SubtreeData> {
        let handle = self.context_client.datastore_handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        let mut entities = Vec::new();

        for (key_result, value_result) in iter.entries() {
            let key = key_result?;
            let value = value_result?;

            if key.context_id() != context_id {
                continue;
            }

            let state_key = key.state_key();
            if !key_shares_subtree_prefix(&state_key, subtree_root, SUBTREE_PREFIX_LEN) {
                continue;
            }

            // Read real CRDT metadata from Index (matching HashComparison responder pattern)
            let metadata = with_runtime_env(runtime_env.clone(), || {
                let entity_id = Id::new(state_key);
                match Index::<MainStorage>::get_index(entity_id) {
                    Ok(Some(idx)) => {
                        let crdt_type = idx
                            .metadata
                            .crdt_type
                            .clone()
                            .unwrap_or_else(|| CrdtType::lww_register("unknown"));
                        LeafMetadata::new(crdt_type, idx.metadata.updated_at(), [0u8; 32])
                    }
                    _ => {
                        // Entity in KV but not in Index — legacy or orphaned data
                        LeafMetadata::new(CrdtType::lww_register("unknown"), 0, [0u8; 32])
                    }
                }
            });

            let leaf = TreeLeafData::new(state_key, value.value.to_vec(), metadata);
            entities.push(leaf);
        }

        let root_hash = {
            let mut h = [0u8; 32];
            // Simple hash: XOR of all entity keys
            for entity in &entities {
                for (i, byte) in entity.key.iter().enumerate() {
                    h[i] ^= byte;
                }
            }
            h
        };

        Ok(SubtreeData::new(*subtree_root, root_hash, entities, depth))
    }

    // =========================================================================
    // Initiator Logic (Phase 1 + Phase 2)
    // =========================================================================

    /// Initiator function for SubtreePrefetch sync.
    ///
    /// 1. Phase 1: Send `SubtreePrefetchRequest` with empty roots (discovery),
    ///    receive prefix list, identify divergent subtrees.
    /// 2. Phase 2: Send `SubtreePrefetchRequest` with divergent roots, receive
    ///    full subtree data.
    /// 3. Cleanup stale local keys + apply received entities.
    /// 4. On any failure: fall back to snapshot sync.
    pub(crate) async fn request_subtree_prefetch(
        &self,
        context_id: ContextId,
        our_identity: calimero_primitives::identity::PublicKey,
        peer_id: libp2p::PeerId,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        // Phase 1: Discover remote prefixes via SubtreePrefetchRequest with empty roots
        info!(%context_id, "SubtreePrefetch Phase 1: discovering remote prefixes");

        let phase1_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::SubtreePrefetchRequest {
                context_id,
                subtree_roots: vec![],
                max_depth: None,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        if let Err(e) = super::stream::send(stream, &phase1_msg, None).await {
            warn!(%context_id, error = %e, "SubtreePrefetch Phase 1 send failed, falling back to snapshot");
            return self
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                .await;
        }

        let response = match self.recv(stream, None).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                warn!(%context_id, "SubtreePrefetch Phase 1: no response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
            Err(e) => {
                warn!(%context_id, error = %e, "SubtreePrefetch Phase 1 recv failed, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        // Parse discovery response: extract remote prefix → hash mapping
        let remote_prefix_hashes: HashMap<u8, [u8; 32]> = match response {
            StreamMessage::Message {
                payload: MessagePayload::SubtreePrefetchResponse { subtrees, .. },
                ..
            } => {
                let discovery: SubtreePrefetchResponse = match borsh::from_slice(&subtrees) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(%context_id, error = %e, "SubtreePrefetch Phase 1: failed to deserialize discovery, falling back to snapshot");
                        return self
                            .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                            .await;
                    }
                };
                discovery
                    .subtrees
                    .iter()
                    .map(|s| (s.root_id[0], s.root_hash))
                    .collect()
            }
            _ => {
                warn!(%context_id, "SubtreePrefetch Phase 1: unexpected response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        // Identify divergent subtrees by comparing remote prefix hashes with local
        let divergent_roots = match self
            .identify_divergent_subtrees(context_id, &remote_prefix_hashes)
        {
            Ok(roots) => roots,
            Err(e) => {
                warn!(%context_id, error = %e, "Failed to identify divergent subtrees, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        if divergent_roots.is_empty() {
            info!(%context_id, "SubtreePrefetch: no divergent subtrees found, already in sync");
            return Ok(SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![],
            });
        }

        info!(
            %context_id,
            divergent_count = divergent_roots.len(),
            "SubtreePrefetch Phase 2: requesting divergent subtrees"
        );

        // Phase 2: Request divergent subtrees
        let phase2_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::SubtreePrefetchRequest {
                context_id,
                subtree_roots: divergent_roots.clone(),
                max_depth: None,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        if let Err(e) = super::stream::send(stream, &phase2_msg, None).await {
            warn!(%context_id, error = %e, "SubtreePrefetch Phase 2 send failed, falling back to snapshot");
            return self
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                .await;
        }

        let phase2_response = match self.recv(stream, None).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                warn!(%context_id, "SubtreePrefetch Phase 2: no response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
            Err(e) => {
                warn!(%context_id, error = %e, "SubtreePrefetch Phase 2 recv failed, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        let subtrees_data = match phase2_response {
            StreamMessage::Message {
                payload: MessagePayload::SubtreePrefetchResponse { subtrees, .. },
                ..
            } => subtrees,
            _ => {
                warn!(%context_id, "SubtreePrefetch Phase 2: unexpected response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        // Deserialize and validate the response
        let prefetch_response: SubtreePrefetchResponse = match borsh::from_slice(&subtrees_data) {
            Ok(r) => r,
            Err(e) => {
                warn!(%context_id, error = %e, "Failed to deserialize SubtreePrefetchResponse, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        if !prefetch_response.is_valid() {
            warn!(%context_id, "SubtreePrefetchResponse failed validation, falling back to snapshot");
            return self
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                .await;
        }

        // Phase 3: Cleanup stale keys + Apply received entities
        //
        // For each divergent subtree, the remote peer's view is authoritative.
        // Local keys under divergent prefixes that are absent from the remote
        // response represent remote deletions and must be removed to ensure
        // convergence. Truncated subtrees are excluded from cleanup since we
        // don't have the complete remote picture for those prefixes.

        // Build lookup: remote entity keys per non-truncated subtree root
        let mut remote_keys_by_root: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        for subtree in &prefetch_response.subtrees {
            if !subtree.is_truncated() {
                remote_keys_by_root.insert(
                    subtree.root_id[0],
                    subtree.entities.iter().map(|e| e.key).collect(),
                );
            }
        }

        // Prefixes in not_found → remote has zero entities under this prefix
        let not_found_prefixes: HashSet<u8> =
            prefetch_response.not_found.iter().map(|r| r[0]).collect();

        // Collect stale local keys (read-only scan, then delete separately)
        let keys_to_delete: Vec<[u8; 32]> = {
            let handle = self.context_client.datastore_handle();
            let mut iter = handle.iter::<ContextStateKey>()?;
            let mut stale = Vec::new();

            for (key_result, _) in iter.entries() {
                let key = key_result?;
                if key.context_id() != context_id {
                    continue;
                }
                let state_key = key.state_key();

                if is_key_stale(
                    &state_key,
                    &divergent_roots,
                    &remote_keys_by_root,
                    &not_found_prefixes,
                    SUBTREE_PREFIX_LEN,
                ) {
                    stale.push(state_key);
                }
            }
            stale
        };

        // Delete stale keys (raw KV — matching snapshot sync cleanup pattern)
        let mut total_deleted = 0usize;
        let mut total_applied = 0usize;
        {
            let mut handle = self.context_client.datastore_handle();
            for state_key in &keys_to_delete {
                handle.delete(&ContextStateKey::new(context_id, *state_key))?;
                total_deleted += 1;
            }
        }

        // Apply received entities with CRDT merge (Invariant I5: No Silent Data Loss).
        // Uses apply_leaf_with_crdt_merge like HashComparison and LevelWise protocols,
        // ensuring proper merge semantics based on crdt_type and HLC timestamps.
        let store = self.context_client.datastore_handle().into_inner();
        let runtime_env = create_runtime_env(&store, context_id, our_identity);

        for subtree in &prefetch_response.subtrees {
            for entity in &subtree.entities {
                with_runtime_env(runtime_env.clone(), || {
                    super::helpers::apply_leaf_with_crdt_merge(context_id, entity)
                })?;
                total_applied += 1;
            }
        }

        info!(
            %context_id,
            total_applied,
            total_deleted,
            subtrees_received = prefetch_response.subtree_count(),
            not_found = prefetch_response.not_found.len(),
            "SubtreePrefetch sync completed"
        );

        Ok(SyncProtocol::SubtreePrefetch {
            subtree_roots: divergent_roots,
        })
    }

    /// Compare remote prefix hashes with local storage to identify divergent subtrees.
    ///
    /// Three categories produce divergent roots:
    /// - Remote-only prefixes: remote has data we don't — must fetch.
    /// - Local-only prefixes: we have data remote doesn't — divergent (may
    ///   indicate remote deletion; Phase 2 will resolve).
    /// - Shared prefixes with **differing** XOR hashes — data differs.
    ///
    /// Shared prefixes with **matching** hashes are skipped (already in sync).
    fn identify_divergent_subtrees(
        &self,
        context_id: ContextId,
        remote_prefix_hashes: &HashMap<u8, [u8; 32]>,
    ) -> eyre::Result<Vec<[u8; 32]>> {
        let handle = self.context_client.datastore_handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        // Single scan: compute local per-prefix XOR hashes (same algorithm as responder).
        // Must include both key and value bytes to detect value-only divergence.
        let mut local_prefix_hashes: HashMap<u8, [u8; 32]> = HashMap::new();
        for (key_result, value_result) in iter.entries() {
            let key = key_result?;
            let value = value_result?;
            if key.context_id() == context_id {
                let state_key = key.state_key();
                let hash = local_prefix_hashes.entry(state_key[0]).or_insert([0u8; 32]);
                xor_into_hash(hash, &state_key);
                xor_into_hash(hash, &value.value);
            }
        }

        let mut divergent = Vec::new();
        let mut skipped = 0u32;

        // Remote-only prefixes (remote has data we lack)
        for &prefix in remote_prefix_hashes.keys() {
            if !local_prefix_hashes.contains_key(&prefix) {
                let mut root = [0u8; 32];
                root[0] = prefix;
                divergent.push(root);
            }
        }

        // Local-only prefixes (we have data remote lacks)
        for &prefix in local_prefix_hashes.keys() {
            if !remote_prefix_hashes.contains_key(&prefix) {
                let mut root = [0u8; 32];
                root[0] = prefix;
                divergent.push(root);
            }
        }

        // Shared prefixes: compare hashes, only include if they differ
        for (&prefix, remote_hash) in remote_prefix_hashes {
            if let Some(local_hash) = local_prefix_hashes.get(&prefix) {
                if local_hash != remote_hash {
                    let mut root = [0u8; 32];
                    root[0] = prefix;
                    divergent.push(root);
                } else {
                    skipped += 1;
                }
            }
        }

        debug!(
            %context_id,
            local_prefix_count = local_prefix_hashes.len(),
            remote_prefix_count = remote_prefix_hashes.len(),
            divergent_count = divergent.len(),
            skipped_matching = skipped,
            "Identified divergent subtrees (hash comparison)"
        );

        Ok(divergent)
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// XOR arbitrary-length data into a 32-byte hash, wrapping with `i % 32`.
///
/// Used by both responder (`send_prefix_discovery`) and initiator
/// (`identify_divergent_subtrees`) to fold key+value bytes into per-prefix
/// divergence hashes. Both sides must use the same algorithm.
fn xor_into_hash(hash: &mut [u8; 32], data: &[u8]) {
    for (i, byte) in data.iter().enumerate() {
        hash[i % 32] ^= byte;
    }
}

/// Check if a key shares the subtree prefix with a given root.
///
/// Compares the first N bytes of the key with the root prefix,
/// where N = min(depth, 32).
fn key_shares_subtree_prefix(key: &[u8; 32], root: &[u8; 32], depth: usize) -> bool {
    let compare_len = depth.min(32);
    if compare_len == 0 {
        return true; // Zero depth matches everything
    }
    key[..compare_len] == root[..compare_len]
}

/// Determine whether a local key is stale and should be deleted.
///
/// A key is stale when it falls under a divergent subtree prefix AND either:
/// - The prefix is in `not_found_prefixes` (remote has no entities there), OR
/// - The key is absent from the remote entity set for that prefix.
///
/// Keys under truncated subtrees should NOT be in `remote_keys_by_root`
/// (caller is responsible for excluding them), so they won't be deleted.
fn is_key_stale(
    state_key: &[u8; 32],
    divergent_roots: &[[u8; 32]],
    remote_keys_by_root: &HashMap<u8, HashSet<[u8; 32]>>,
    not_found_prefixes: &HashSet<u8>,
    depth: usize,
) -> bool {
    for root in divergent_roots {
        if !key_shares_subtree_prefix(state_key, root, depth) {
            continue;
        }
        let prefix = root[0];

        if not_found_prefixes.contains(&prefix) {
            return true;
        }
        if let Some(remote_keys) = remote_keys_by_root.get(&prefix) {
            return !remote_keys.contains(state_key);
        }
        // Matched a divergent root but prefix is neither not_found nor in
        // remote_keys_by_root (e.g. truncated subtree) → not stale
        return false;
    }
    false
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_shares_subtree_prefix_exact_match() {
        let key = [1u8; 32];
        let root = [1u8; 32];
        assert!(key_shares_subtree_prefix(&key, &root, 32));
    }

    #[test]
    fn test_key_shares_subtree_prefix_first_byte_match() {
        let mut key = [0u8; 32];
        key[0] = 0xAB;
        key[1] = 0x01;

        let mut root = [0u8; 32];
        root[0] = 0xAB;
        root[1] = 0xFF;

        // Depth 1: only compare first byte
        assert!(key_shares_subtree_prefix(&key, &root, 1));
        // Depth 2: compare first two bytes (differ at index 1)
        assert!(!key_shares_subtree_prefix(&key, &root, 2));
    }

    #[test]
    fn test_key_shares_subtree_prefix_zero_depth() {
        let key = [0u8; 32];
        let root = [0xFF; 32];
        // Zero depth matches everything
        assert!(key_shares_subtree_prefix(&key, &root, 0));
    }

    #[test]
    fn test_key_shares_subtree_prefix_large_depth() {
        let key = [0xAA; 32];
        let root = [0xAA; 32];
        // Depth > 32 is clamped to 32
        assert!(key_shares_subtree_prefix(&key, &root, 100));
    }

    #[test]
    fn test_key_shares_subtree_prefix_mismatch() {
        let key = [1u8; 32];
        let root = [2u8; 32];
        assert!(!key_shares_subtree_prefix(&key, &root, 1));
    }

    #[test]
    fn test_key_shares_subtree_prefix_partial_match() {
        let mut key = [0u8; 32];
        key[0] = 0x01;
        key[1] = 0x02;
        key[2] = 0x03;

        let mut root = [0u8; 32];
        root[0] = 0x01;
        root[1] = 0x02;
        root[2] = 0xFF; // Differs at byte 2

        assert!(key_shares_subtree_prefix(&key, &root, 2)); // Only checks bytes 0-1
        assert!(!key_shares_subtree_prefix(&key, &root, 3)); // Checks bytes 0-2
    }

    // =========================================================================
    // is_key_stale Tests — stale key identification for subtree cleanup
    // =========================================================================

    /// Helper: build a divergent root from a prefix byte.
    fn root_from_prefix(prefix: u8) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[0] = prefix;
        r
    }

    /// Helper: build a state key with given prefix byte and second byte.
    fn key_with_prefix(prefix: u8, second: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[0] = prefix;
        k[1] = second;
        k
    }

    #[test]
    fn test_is_key_stale_key_absent_from_remote_set() {
        // Remote has keys {A, B} under prefix 0x01; local has key C → C is stale
        let divergent_roots = vec![root_from_prefix(0x01)];
        let mut remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        remote_keys.insert(
            0x01,
            HashSet::from([key_with_prefix(0x01, 0xAA), key_with_prefix(0x01, 0xBB)]),
        );
        let not_found: HashSet<u8> = HashSet::new();

        let local_key = key_with_prefix(0x01, 0xCC); // not in remote set
        assert!(is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1, // depth 1: match on first byte
        ));
    }

    #[test]
    fn test_is_key_stale_key_present_in_remote_set() {
        // Remote has key A; local also has A → not stale
        let divergent_roots = vec![root_from_prefix(0x01)];
        let mut remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        remote_keys.insert(0x01, HashSet::from([key_with_prefix(0x01, 0xAA)]));
        let not_found: HashSet<u8> = HashSet::new();

        let local_key = key_with_prefix(0x01, 0xAA); // in remote set
        assert!(!is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_prefix_in_not_found() {
        // Prefix 0x02 is in not_found → all local keys under 0x02 are stale
        let divergent_roots = vec![root_from_prefix(0x02)];
        let remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        let not_found: HashSet<u8> = HashSet::from([0x02]);

        let local_key = key_with_prefix(0x02, 0x05);
        assert!(is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_key_under_non_divergent_prefix() {
        // Key under prefix 0x03, but only 0x01 is divergent → not stale
        let divergent_roots = vec![root_from_prefix(0x01)];
        let remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        let not_found: HashSet<u8> = HashSet::new();

        let local_key = key_with_prefix(0x03, 0x01);
        assert!(!is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_truncated_subtree_excluded() {
        // Prefix 0x01 is divergent but NOT in remote_keys_by_root (truncated)
        // and NOT in not_found → key is not stale (we lack the full picture)
        let divergent_roots = vec![root_from_prefix(0x01)];
        let remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        let not_found: HashSet<u8> = HashSet::new();

        let local_key = key_with_prefix(0x01, 0x42);
        assert!(!is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_multiple_divergent_roots() {
        // Two divergent roots: 0x01 (has remote keys), 0x02 (not found)
        let divergent_roots = vec![root_from_prefix(0x01), root_from_prefix(0x02)];
        let mut remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        remote_keys.insert(0x01, HashSet::from([key_with_prefix(0x01, 0xAA)]));
        let not_found: HashSet<u8> = HashSet::from([0x02]);

        // Key under 0x01 present in remote → not stale
        assert!(!is_key_stale(
            &key_with_prefix(0x01, 0xAA),
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
        // Key under 0x01 absent from remote → stale
        assert!(is_key_stale(
            &key_with_prefix(0x01, 0xBB),
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
        // Key under 0x02 (not found prefix) → stale
        assert!(is_key_stale(
            &key_with_prefix(0x02, 0x01),
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_empty_remote_set() {
        // Remote returns a subtree with zero entities (but it's non-truncated,
        // so it shows up in remote_keys_by_root with an empty set)
        let divergent_roots = vec![root_from_prefix(0x01)];
        let mut remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        remote_keys.insert(0x01, HashSet::new()); // empty set
        let not_found: HashSet<u8> = HashSet::new();

        // Any local key under 0x01 is stale (remote has nothing)
        let local_key = key_with_prefix(0x01, 0x01);
        assert!(is_key_stale(
            &local_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            1,
        ));
    }

    #[test]
    fn test_is_key_stale_depth_limits_scope() {
        // With depth=5, root [0x01, 0, 0, 0, 0, ...] only matches keys
        // whose first 5 bytes are [0x01, 0, 0, 0, 0].
        let divergent_roots = vec![root_from_prefix(0x01)];
        let mut remote_keys: HashMap<u8, HashSet<[u8; 32]>> = HashMap::new();
        remote_keys.insert(0x01, HashSet::new());
        let not_found: HashSet<u8> = HashSet::new();

        // Key [0x01, 0, 0, 0, 0, ...] matches → stale (not in remote set)
        let matching_key = key_with_prefix(0x01, 0x00);
        assert!(is_key_stale(
            &matching_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            5,
        ));

        // Key [0x01, 1, 0, 0, 0, ...] does NOT match at depth=5 → not stale
        let non_matching_key = key_with_prefix(0x01, 0x01);
        assert!(!is_key_stale(
            &non_matching_key,
            &divergent_roots,
            &remote_keys,
            &not_found,
            5,
        ));
    }

    // =========================================================================
    // xor_into_hash Tests — divergence hash used by discovery & identification
    // =========================================================================

    #[test]
    fn test_xor_into_hash_basic() {
        let mut hash = [0u8; 32];
        xor_into_hash(&mut hash, &[0xFF]);
        assert_eq!(hash[0], 0xFF);
        assert_eq!(hash[1], 0x00); // untouched bytes stay zero
    }

    #[test]
    fn test_xor_into_hash_self_inverse() {
        // XOR-ing the same data twice cancels out (returns to zero)
        let mut hash = [0u8; 32];
        let data = [0xAB, 0xCD, 0xEF];
        xor_into_hash(&mut hash, &data);
        xor_into_hash(&mut hash, &data);
        assert_eq!(hash, [0u8; 32]);
    }

    #[test]
    fn test_xor_into_hash_wraps_at_32() {
        // Data longer than 32 bytes wraps via `i % 32`
        let mut hash = [0u8; 32];
        let mut data = [0u8; 33];
        data[0] = 0x01;
        data[32] = 0x02; // wraps to index 0
        xor_into_hash(&mut hash, &data);
        assert_eq!(hash[0], 0x01 ^ 0x02); // both folded into byte 0
    }

    #[test]
    fn test_xor_into_hash_order_independent() {
        // XOR is commutative: hash(A, B) == hash(B, A)
        let a = [0x11, 0x22];
        let b = [0x33, 0x44];

        let mut hash_ab = [0u8; 32];
        xor_into_hash(&mut hash_ab, &a);
        xor_into_hash(&mut hash_ab, &b);

        let mut hash_ba = [0u8; 32];
        xor_into_hash(&mut hash_ba, &b);
        xor_into_hash(&mut hash_ba, &a);

        assert_eq!(hash_ab, hash_ba);
    }

    #[test]
    fn test_xor_into_hash_empty_data_is_noop() {
        let mut hash = [0x42; 32];
        let before = hash;
        xor_into_hash(&mut hash, &[]);
        assert_eq!(hash, before);
    }

    // =========================================================================
    // Note on CRDT merge testing
    // =========================================================================
    //
    // The CRDT-aware entity application path (`apply_leaf_with_crdt_merge`
    // within `with_runtime_env`) requires a full storage runtime environment
    // (RuntimeEnv, Index, MainStorage). It cannot be unit tested here.
    //
    // It is tested indirectly through the sync_sim integration tests which
    // set up SimStorage with proper storage backends.
    // See: crates/node/tests/sync_sim/
}
