//! Sync handshake construction for [`SyncManager`]: building the local and
//! remote `SyncHandshake` summaries (entity count + tree depth, with a
//! DAG-heads fallback) used for protocol negotiation. Extracted from the
//! manager god-file as an `impl SyncManager` fragment.

use calimero_node_primitives::sync::{
    build_handshake_from_raw, estimate_entity_count, estimate_max_depth, SyncHandshake,
};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;

use super::SyncManager;

impl SyncManager {
    /// Build `SyncHandshake` from local context state for protocol negotiation.
    ///
    /// Queries the real entity count and tree depth from the Merkle tree Index
    /// via the storage bridge. Falls back to estimation from DAG heads if the
    /// Index is not accessible (e.g., after snapshot sync with format mismatch).
    ///
    /// # Arguments
    ///
    /// * `context` - The context to build a handshake for.
    ///
    /// # Returns
    ///
    /// A `SyncHandshake` containing the context's current state summary.
    pub(super) fn build_local_handshake(
        &self,
        context: &calimero_primitives::context::Context,
    ) -> SyncHandshake {
        let root_hash = *context.root_hash;
        let dag_heads = context.dag_heads.clone();

        // Try to get real entity count and depth from the Merkle tree Index.
        // This gives accurate protocol selection instead of guessing from dag_heads.
        let (entity_count, max_depth) = self.query_tree_stats(&context.id).unwrap_or_else(|| {
            // Fallback: estimate from dag_heads if Index is unavailable
            let count = estimate_entity_count(root_hash, dag_heads.len());
            let depth = estimate_max_depth(count);
            (count, depth)
        });

        build_handshake_from_raw(root_hash, entity_count, max_depth, dag_heads)
    }

    /// Query real entity count and tree depth from the Merkle tree Index.
    ///
    /// Returns `Some((entity_count, max_depth))` on success, `None` if the
    /// Index is unavailable (e.g., fresh node or deserialization mismatch).
    fn query_tree_stats(&self, context_id: &ContextId) -> Option<(u64, u32)> {
        use calimero_node_primitives::sync::create_runtime_env;
        use calimero_storage::address::Id;
        use calimero_storage::env::with_runtime_env;
        use calimero_storage::index::Index;
        use calimero_storage::store::MainStorage;

        let store = self.context_client.datastore_handle().into_inner();
        // SAFETY: identity is unused for read-only Index queries via RuntimeEnv
        let identity = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        let env = create_runtime_env(&store, *context_id, identity);

        let root_id = Id::new(*context_id.as_ref());

        with_runtime_env(env, || {
            // Check if root Index exists
            let root_index = Index::<MainStorage>::get_index(root_id).ok().flatten()?;

            // Count children (leaf entities) under root.
            // Minimum 1 when root exists (consistent with fallback estimation).
            let children = root_index.children().unwrap_or_default();
            let entity_count = (children.len() as u64).max(1);

            // Depth: 1 when root has data (consistent with fallback).
            // For deeper trees, we'd need recursive traversal — tracked in #2054.
            let max_depth = 1;

            Some((entity_count, max_depth))
        })
    }

    /// Build `SyncHandshake` from peer state for protocol negotiation.
    ///
    /// Uses shared estimation functions from `calimero_node_primitives::sync::state_machine`
    /// to ensure consistent behavior between production (`SyncManager`) and simulation (`SimNode`).
    pub(super) fn build_remote_handshake(
        peer_root_hash: calimero_primitives::hash::Hash,
        peer_dag_heads: &[[u8; DIGEST_SIZE]],
    ) -> SyncHandshake {
        let root_hash = *peer_root_hash;

        // Use shared estimation functions for consistency with simulation
        let entity_count = estimate_entity_count(root_hash, peer_dag_heads.len());
        let max_depth = estimate_max_depth(entity_count);

        build_handshake_from_raw(root_hash, entity_count, max_depth, peer_dag_heads.to_vec())
    }
}
