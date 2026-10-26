//! Sync handshake construction for [`SyncManager`]: building the local and
//! remote `SyncHandshake` summaries (entity count + tree depth, with a
//! DAG-heads fallback) used for protocol negotiation. Extracted from the
//! manager god-file as an `impl SyncManager` fragment.

use calimero_node_primitives::sync::DEEP_TREE_DEPTH;
use calimero_node_primitives::sync::{
    build_handshake_from_raw, estimate_entity_count, estimate_max_depth, SyncHandshake,
};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::index::Index;
use calimero_storage::store::{MainStorage, StorageAdaptor};

use super::SyncManager;

const TREE_STATS_MAX_NODES: usize = 1000; // bounds the children read per handshake

/// Levels under `id` (a lone node is 1), counted down to `cap`; a node too wide
/// for the remaining `nodes_left` budget counts as one level, unexpanded.
fn capped_depth<S: StorageAdaptor>(id: Id, cap: u32, nodes_left: &mut usize) -> u32 {
    let width = Index::<S>::child_count(id) as usize;
    if cap <= 1 || width == 0 {
        return 1;
    }
    if width > *nodes_left {
        return 2;
    }
    *nodes_left -= width;
    let children = Index::<S>::get_children_of(id).unwrap_or_default();
    let deepest = children
        .iter()
        .map(|c| capped_depth::<S>(c.id(), cap - 1, nodes_left));
    1 + deepest.max().unwrap_or(0)
}

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
        use calimero_storage::env::with_runtime_env;

        let store = self.context_client.datastore_handle().into_inner();
        // SAFETY: identity is unused for read-only Index queries via RuntimeEnv
        let identity = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        // `None` rather than `?`: this is a read-only index query in an
        // Option-returning helper, so an unresolvable account means "no answer",
        // not a failure to propagate.
        let account = calimero_governance_store::account_for_context(&store, context_id).ok()?;
        let env = create_runtime_env(&store, *context_id, identity, account);

        let root_id = Id::new(*context_id.as_ref());

        with_runtime_env(env, || {
            // Check if root Index exists
            let root_index = Index::<MainStorage>::get_index(root_id).ok().flatten()?;

            // Count children (leaf entities) under root.
            // Minimum 1 when root exists (consistent with fallback estimation).
            let _ = &root_index;
            // The id this row was read under, not `Id::root()`. `Id::root()`
            // takes `context_id` from the `RUNTIME_ENV` thread-local, which is
            // installed here today but silently yields a different id if that
            // ever stops being true — and the failure is a count of 1, not an
            // error.
            // Only the count is wanted, and the trie maintains one along the
            // spine precisely so this is a single row read rather than a walk.
            let entity_count = calimero_storage::index::Index::<
                calimero_storage::store::MainStorage,
            >::child_count(root_id)
            .max(1);

            let max_depth = capped_depth::<MainStorage>(
                root_id,
                DEEP_TREE_DEPTH + 1,
                &mut TREE_STATS_MAX_NODES.clone(),
            );

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

#[cfg(test)]
mod tests {
    use calimero_storage::action::Action;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{ChildInfo, Metadata};
    use calimero_storage::index::Index;
    use calimero_storage::interface::{ApplyContext, Interface};
    use calimero_storage::store::MockedStorage;

    use super::capped_depth;

    type S = MockedStorage<9001>;

    fn root() -> ChildInfo {
        Index::<S>::add_root(ChildInfo::new(
            Id::new([0; 32]),
            [0; 32],
            Metadata::default(),
        ))
        .unwrap();
        let (hash, _) = Index::<S>::get_hashes_for(Id::new([0; 32]))
            .unwrap()
            .unwrap();
        ChildInfo::new(Id::new([0; 32]), hash, Metadata::default())
    }

    /// Adds `id` under the chain `ancestors` (nearest parent first).
    fn add(id: u8, ancestors: Vec<ChildInfo>) -> ChildInfo {
        let action = Action::Update {
            id: Id::new([id; 32]),
            data: vec![id],
            ancestors,
            metadata: Metadata::default(),
        };
        Interface::<S>::apply_action(action, &ApplyContext::empty()).unwrap();
        let (hash, _) = Index::<S>::get_hashes_for(Id::new([id; 32]))
            .unwrap()
            .unwrap();
        ChildInfo::new(Id::new([id; 32]), hash, Metadata::default())
    }

    #[test]
    fn a_flat_tree_is_two_levels() {
        let root = root();
        add(1, vec![root.clone()]);
        add(2, vec![root]);
        assert_eq!(capped_depth::<S>(Id::new([0; 32]), 4, &mut 1000), 2);
    }

    #[test]
    fn a_deep_tree_stops_at_the_cap() {
        let root = root();
        let a = add(1, vec![root.clone()]);
        let b = add(2, vec![a.clone(), root.clone()]);
        let c = add(3, vec![b.clone(), a.clone(), root.clone()]);
        add(4, vec![c, b, a, root]);
        assert_eq!(capped_depth::<S>(Id::new([0; 32]), 4, &mut 1000), 4);
        assert_eq!(capped_depth::<S>(Id::new([0; 32]), 3, &mut 1000), 3);
    }
}
