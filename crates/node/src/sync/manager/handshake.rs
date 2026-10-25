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

const TREE_STATS_MAX_NODES: usize = 1000; // bounds the depth sample taken per handshake

/// Levels in the tree under `root` (a lone root is 1), sampled over at most
/// `max_nodes` nodes. `children(id, budget)` lists a node's children, or
/// `None` when it has more than `budget`: such a node still counts one more
/// level but is not expanded, so the result is a lower bound.
fn sampled_depth(
    root: calimero_storage::address::Id,
    max_nodes: usize,
    children: impl Fn(
        calimero_storage::address::Id,
        usize,
    ) -> Option<Vec<calimero_storage::address::Id>>,
) -> u32 {
    let mut level = vec![root];
    let mut seen = 1;
    let mut depth = 1;
    loop {
        let mut next = Vec::new();
        let mut truncated = false;
        for id in &level {
            match children(*id, max_nodes.saturating_sub(seen + next.len())) {
                Some(ids) => next.extend(ids),
                None => truncated = true,
            }
        }
        if next.is_empty() && !truncated {
            return depth;
        }
        depth += 1;
        seen += next.len();
        if depth as usize >= calimero_node_primitives::sync::MAX_LEVELWISE_DEPTH {
            return depth;
        }
        level = next;
    }
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
        use calimero_storage::address::Id;
        use calimero_storage::env::with_runtime_env;
        use calimero_storage::index::Index;
        use calimero_storage::store::MainStorage;

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

            let max_depth = sampled_depth(root_id, TREE_STATS_MAX_NODES, |id, budget| {
                if Index::<MainStorage>::child_count(id) as usize > budget {
                    return None;
                }
                let children = Index::<MainStorage>::get_children_of(id).unwrap_or_default();
                Some(children.iter().map(|child| child.id()).collect())
            });

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
    use std::collections::HashMap;

    use calimero_storage::address::Id;

    use super::sampled_depth;

    fn id(n: u8) -> Id {
        Id::new([n; 32])
    }

    /// Children lookup over a fixed tree, refusing to list a node with more
    /// children than the budget, as the Index-backed lookup does.
    fn lookup(tree: &HashMap<Id, Vec<Id>>) -> impl Fn(Id, usize) -> Option<Vec<Id>> + '_ {
        move |at, budget| {
            let children = tree.get(&at).cloned().unwrap_or_default();
            (children.len() <= budget).then_some(children)
        }
    }

    #[test]
    fn a_root_alone_is_one_level() {
        let tree = HashMap::new();
        assert_eq!(sampled_depth(id(0), 1000, lookup(&tree)), 1);
    }

    #[test]
    fn a_flat_tree_is_two_levels() {
        let tree = HashMap::from([(id(0), vec![id(1), id(2), id(3)])]);
        assert_eq!(sampled_depth(id(0), 1000, lookup(&tree)), 2);
    }

    #[test]
    fn a_three_level_tree_is_deeper_than_one() {
        let tree = HashMap::from([
            (id(0), vec![id(1), id(2)]),
            (id(1), vec![id(3)]),
            (id(3), vec![id(4)]),
        ]);
        assert_eq!(sampled_depth(id(0), 1000, lookup(&tree)), 4);
    }

    #[test]
    fn a_node_too_wide_for_the_budget_still_counts_a_level() {
        let tree = HashMap::from([(id(0), (1..=5).map(id).collect())]);
        assert_eq!(sampled_depth(id(0), 3, lookup(&tree)), 2);
    }

    #[test]
    fn the_walk_stops_at_the_level_cap() {
        let tree: HashMap<Id, Vec<Id>> = (0..200).map(|n| (id(n), vec![id(n + 1)])).collect();
        assert_eq!(
            sampled_depth(id(0), 1000, lookup(&tree)) as usize,
            calimero_node_primitives::sync::MAX_LEVELWISE_DEPTH
        );
    }
}
