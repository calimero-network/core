//! SubtreePrefetch sync protocol implementation.
//!
//! Two-phase protocol for efficient synchronization of deep trees with clustered changes:
//! - Phase 1: Compare top-level tree node hashes to find divergent subtrees
//! - Phase 2: Fetch entire divergent subtrees in one request
//!
//! Falls back to snapshot sync on any failure.

use std::collections::HashSet;

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::hash_comparison::{CrdtType, LeafMetadata, TreeLeafData};
use calimero_node_primitives::sync::subtree::{SubtreeData, SubtreePrefetchResponse};
use calimero_node_primitives::sync::{
    InitPayload, MessagePayload, StreamMessage, SyncProtocol,
};
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
    /// Handle a TreeNodeRequest (Phase 1 of SubtreePrefetch).
    ///
    /// Enumerates unique first-byte prefixes of state keys as "top-level children"
    /// and returns them as `Vec<([u8; 32], bool)>` where the hash is derived from
    /// the prefix byte and `is_leaf` is always false (they are subtree roots).
    pub(crate) async fn handle_tree_node_request(
        &self,
        context_id: ContextId,
        _node_id: [u8; 32],
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

        // Build children: one entry per unique first-byte prefix
        let children: Vec<([u8; 32], bool)> = seen_prefixes
            .into_iter()
            .map(|prefix_byte| {
                let mut hash = [0u8; 32];
                hash[0] = prefix_byte;
                (hash, false) // is_leaf = false, these are subtree roots
            })
            .collect();

        debug!(
            %context_id,
            children_count = children.len(),
            "Responding to TreeNodeRequest with top-level children"
        );

        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::TreeNodeResponse { children },
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
        let depth = max_depth
            .unwrap_or(calimero_node_primitives::sync::subtree::DEFAULT_SUBTREE_MAX_DEPTH);

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
        let serialized = borsh::to_vec(&response)
            .wrap_err("failed to serialize SubtreePrefetchResponse")?;

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

            let metadata = LeafMetadata::new(CrdtType::LwwRegister, 0, [0u8; 32]);
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

        Ok(SubtreeData::new(
            *subtree_root,
            root_hash,
            entities,
            depth,
        ))
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
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        if let Err(e) = super::stream::send(stream, &phase1_msg, None).await {
            warn!(%context_id, error = %e, "SubtreePrefetch Phase 1 send failed, falling back to snapshot");
            return self
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                .await;
        }

        let response = match self.recv(stream, None).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                warn!(%context_id, "SubtreePrefetch Phase 1: no response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                    .await;
            }
            Err(e) => {
                warn!(%context_id, error = %e, "SubtreePrefetch Phase 1 recv failed, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                    .await;
            }
        };

        let remote_children = match response {
            StreamMessage::Message {
                payload: MessagePayload::TreeNodeResponse { children },
                ..
            } => children,
            _ => {
                warn!(%context_id, "SubtreePrefetch Phase 1: unexpected response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                    .await;
            }
        };

        // Identify divergent subtrees by comparing remote children with local storage
        let divergent_roots = match self.identify_divergent_subtrees(context_id, &remote_children) {
            Ok(roots) => roots,
            Err(e) => {
                warn!(%context_id, error = %e, "Failed to identify divergent subtrees, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
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
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                .await;
        }

        let phase2_response = match self.recv(stream, None).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                warn!(%context_id, "SubtreePrefetch Phase 2: no response, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                    .await;
            }
            Err(e) => {
                warn!(%context_id, error = %e, "SubtreePrefetch Phase 2 recv failed, falling back to snapshot");
                return self
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
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
                    .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                    .await;
            }
        };

        // Deserialize and validate the response
        let prefetch_response: SubtreePrefetchResponse =
            match borsh::from_slice(&subtrees_data) {
                Ok(r) => r,
                Err(e) => {
                    warn!(%context_id, error = %e, "Failed to deserialize SubtreePrefetchResponse, falling back to snapshot");
                    return self
                        .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                        .await;
                }
            };

        if !prefetch_response.is_valid() {
            warn!(%context_id, "SubtreePrefetchResponse failed validation, falling back to snapshot");
            return self
                .fallback_to_snapshot_sync(context_id, our_identity, peer_id, stream)
                .await;
        }

        // Apply entities to local storage
        let mut total_applied = 0;
        let mut handle = self.context_client.datastore_handle();

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
            subtrees_received = prefetch_response.subtree_count(),
            not_found = prefetch_response.not_found.len(),
            "SubtreePrefetch sync completed"
        );

        Ok(SyncProtocol::SubtreePrefetch {
            subtree_roots: divergent_roots,
        })
    }

    /// Compare remote children with local storage to identify divergent subtrees.
    ///
    /// For each remote child (prefix hash), check if we have the same prefix locally.
    /// Any prefix present remotely but not locally (or vice versa) is considered divergent.
    fn identify_divergent_subtrees(
        &self,
        context_id: ContextId,
        remote_children: &[([u8; 32], bool)],
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

        let remote_prefixes: HashSet<u8> = remote_children
            .iter()
            .map(|(hash, _)| hash[0])
            .collect();

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
}
