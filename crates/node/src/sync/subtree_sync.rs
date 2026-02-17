//! SubtreePrefetch sync protocol implementation.
//!
//! Two-phase protocol for efficient synchronization of deep trees with clustered changes:
//! - Phase 1: Compare top-level tree node hashes to find divergent subtrees
//! - Phase 2: Fetch entire divergent subtrees in one request
//!
//! Falls back to snapshot sync on any failure.

use std::collections::{HashMap, HashSet};

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::hash_comparison::{
    CrdtType, LeafMetadata, TreeLeafData, TreeNode,
};
use calimero_node_primitives::sync::subtree::{SubtreeData, SubtreePrefetchResponse};
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage, SyncProtocol};
use calimero_primitives::context::ContextId;
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use eyre::WrapErr;
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

// =============================================================================
// Responder Handlers
// =============================================================================

impl SyncManager {
    /// Handle a TreeNodeRequest for SubtreePrefetch Phase 1.
    ///
    /// Enumerates unique first-byte prefixes of state keys as "top-level children"
    /// and returns them as `Vec<TreeNode>` where each node's ID is derived from
    /// the prefix byte (internal nodes representing subtree roots).
    pub(crate) async fn handle_subtree_tree_node_request(
        &self,
        context_id: ContextId,
        _node_id: [u8; 32],
        _max_depth: Option<u8>,
        stream: &mut Stream,
        _nonce: calimero_crypto::Nonce,
    ) -> eyre::Result<()> {
        let handle = self.context_client.datastore_handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        let mut seen_prefixes: HashSet<u8> = HashSet::new();

        for (key_result, _) in iter.entries() {
            let key = key_result?;
            if key.context_id() == context_id {
                let state_key = key.state_key();
                seen_prefixes.insert(state_key[0]);
            }
        }

        // Build nodes: one internal TreeNode per unique first-byte prefix
        let nodes: Vec<TreeNode> = seen_prefixes
            .into_iter()
            .map(|prefix_byte| {
                let mut id = [0u8; 32];
                id[0] = prefix_byte;
                // Internal node: ID is prefix-based, hash is zeroed (no Merkle tree),
                // children list is empty (we only report the top-level grouping)
                TreeNode::internal(id, [0u8; 32], vec![])
            })
            .collect();

        debug!(
            %context_id,
            node_count = nodes.len(),
            "Responding to TreeNodeRequest with top-level subtree nodes"
        );

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::TreeNodeResponse {
                nodes,
                not_found: false,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle a SubtreePrefetchRequest (Phase 2 of SubtreePrefetch).
    ///
    /// Walks storage for entities matching the requested subtree roots by key prefix,
    /// and sends back a SubtreePrefetchResponse.
    pub(crate) async fn handle_subtree_prefetch_request(
        &self,
        context_id: ContextId,
        subtree_roots: Vec<[u8; 32]>,
        max_depth: Option<usize>,
        stream: &mut Stream,
        _nonce: calimero_crypto::Nonce,
    ) -> eyre::Result<()> {
        let depth =
            max_depth.unwrap_or(calimero_node_primitives::sync::subtree::DEFAULT_SUBTREE_MAX_DEPTH);

        info!(
            %context_id,
            subtree_count = subtree_roots.len(),
            depth,
            "Handling SubtreePrefetchRequest"
        );

        let mut subtrees = Vec::new();
        let mut not_found = Vec::new();

        for root in &subtree_roots {
            match self.collect_subtree_entities(context_id, root, depth) {
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
    /// Iterates storage, filters by context_id and prefix match, and builds
    /// `TreeLeafData` for each matching entity.
    fn collect_subtree_entities(
        &self,
        context_id: ContextId,
        subtree_root: &[u8; 32],
        depth: usize,
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
            if !key_shares_subtree_prefix(&state_key, subtree_root, depth) {
                continue;
            }

            let metadata = LeafMetadata::new(CrdtType::lww_register("unknown"), 0, [0u8; 32]);
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
    /// 1. Phase 1: Send TreeNodeRequest, receive TreeNodeResponse, find divergent roots
    /// 2. Phase 2: Send SubtreePrefetchRequest, receive SubtreePrefetchResponse
    /// 3. Deserialize response, validate, apply entities via handle.put()
    /// 4. On any failure: fall back to snapshot sync
    pub(crate) async fn request_subtree_prefetch(
        &self,
        context_id: ContextId,
        our_identity: calimero_primitives::identity::PublicKey,
        peer_id: libp2p::PeerId,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        // Get local root hash
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;
        let local_root_hash = *context.root_hash;

        // Phase 1: Request top-level tree node children
        info!(%context_id, "SubtreePrefetch Phase 1: requesting tree node children");

        let phase1_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::TreeNodeRequest {
                context_id,
                node_id: local_root_hash,
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

        let remote_nodes = match response {
            StreamMessage::Message {
                payload: MessagePayload::TreeNodeResponse { nodes, not_found },
                ..
            } => {
                if not_found {
                    warn!(%context_id, "SubtreePrefetch Phase 1: peer reported not_found, falling back to snapshot");
                    return self
                        .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                        .await;
                }
                nodes
            }
            _ => {
                warn!(%context_id, "SubtreePrefetch Phase 1: unexpected response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
                    .await;
            }
        };

        // Identify divergent subtrees by comparing remote children with local storage
        let divergent_roots = match self.identify_divergent_subtrees(context_id, &remote_nodes) {
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
        let depth_used = calimero_node_primitives::sync::subtree::DEFAULT_SUBTREE_MAX_DEPTH;

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
                    depth_used,
                ) {
                    stale.push(state_key);
                }
            }
            stale
        };

        // Apply: delete stale keys, then put received entities
        let mut total_deleted = 0usize;
        let mut total_applied = 0usize;
        let mut handle = self.context_client.datastore_handle();

        for state_key in &keys_to_delete {
            handle.delete(&ContextStateKey::new(context_id, *state_key))?;
            total_deleted += 1;
        }

        for subtree in &prefetch_response.subtrees {
            for entity in &subtree.entities {
                let key = ContextStateKey::new(context_id, entity.key);
                let slice: Slice<'_> = Slice::from(entity.value.clone());
                handle.put(&key, &ContextStateValue::from(slice))?;
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

    /// Compare remote top-level nodes with local storage to identify divergent subtrees.
    ///
    /// For each remote node (keyed by first-byte prefix), check if we have the
    /// same prefix locally. Any prefix present remotely but not locally (or vice
    /// versa) is considered divergent. Shared prefixes are also included since
    /// we cannot compare hashes without a true Merkle tree.
    fn identify_divergent_subtrees(
        &self,
        context_id: ContextId,
        remote_nodes: &[TreeNode],
    ) -> eyre::Result<Vec<[u8; 32]>> {
        let handle = self.context_client.datastore_handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        // Collect local first-byte prefixes
        let mut local_prefixes: HashSet<u8> = HashSet::new();
        for (key_result, _) in iter.entries() {
            let key = key_result?;
            if key.context_id() == context_id {
                let state_key = key.state_key();
                local_prefixes.insert(state_key[0]);
            }
        }

        let remote_prefixes: HashSet<u8> = remote_nodes.iter().map(|node| node.id[0]).collect();

        // Divergent = present in remote but not local, or in local but not remote
        let mut divergent = Vec::new();

        // Prefixes in remote but not in local
        for prefix in &remote_prefixes {
            if !local_prefixes.contains(prefix) {
                let mut root = [0u8; 32];
                root[0] = *prefix;
                divergent.push(root);
            }
        }

        // Prefixes in local but not in remote (also divergent)
        for prefix in &local_prefixes {
            if !remote_prefixes.contains(prefix) {
                let mut root = [0u8; 32];
                root[0] = *prefix;
                divergent.push(root);
            }
        }

        // For prefixes present in both, they may still differ but we treat them as
        // potentially divergent since we cannot compare hashes at this level without
        // a true Merkle tree. Include all remote prefixes that we also have.
        for prefix in remote_prefixes.intersection(&local_prefixes) {
            let mut root = [0u8; 32];
            root[0] = *prefix;
            divergent.push(root);
        }

        debug!(
            %context_id,
            local_prefix_count = local_prefixes.len(),
            remote_prefix_count = remote_prefixes.len(),
            divergent_count = divergent.len(),
            "Identified divergent subtrees"
        );

        Ok(divergent)
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

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
}
