//! Predicate-bounded walk over the descendant group tree, used by both
//! `CascadeTargetApplicationSet` and `CascadeGroupMigrationSet` apply
//! arms (see [`crate::cascade`]).
//!
//! The walk is **read-only**: it materializes the per-descendant
//! `(group_id, matched?)` decision so the caller can iterate it and
//! issue the per-group settings mutation + per-context propagator
//! enqueue. No mutation happens inside this module.

use std::collections::HashSet;

use calimero_context_config::types::ContextGroupId;
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::{MetaRepository, NamespaceRepository};

/// One entry in a [`walk_for_predicate`] result.
///
/// `matched` records the result of the `from_app_key == GroupMeta.app_key`
/// predicate at walk time. The signed group itself is always emitted (so
/// the apply handler can apply the op to the root of the cascade) — its
/// `matched` is computed exactly the same way as any descendant's, with
/// no carve-out, so a signed group that has already been cascaded by a
/// concurrent op is correctly skipped (per spec §5 concurrent-cascade
/// safety).
///
/// Groups whose `GroupMeta` row is missing (registered in the parent
/// child-index but not yet materialized — e.g. a fresh peer that hasn't
/// caught up on the namespace governance DAG) are emitted with
/// `matched = false`. Treating a missing meta as "doesn't match" rather
/// than as an error keeps the cascade apply liveness-correct on
/// catching-up peers: the row will arrive in a subsequent governance
/// round and the matching descendant will be picked up by the next
/// cascade op (the predicate `from_app_key == X` doesn't change just
/// because a peer fell behind). A `from_app_key == [0; 32]` op against
/// such a peer would still legitimately fail to match — `[0; 32]` is
/// not a real app_key.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub group_id: ContextGroupId,
    pub matched: bool,
    /// The descendant's actual `app_key` value at walk time (or
    /// `[0u8; 32]` if no `GroupMeta` was loadable for the group).
    /// Surfaced primarily for diagnostic logging in the cascade apply
    /// arms: when `matched == false`, this lets the skip log identify
    /// *which* value the predicate compared against, distinguishing
    /// "subtree app_key was never set" from "subtree was already at a
    /// different version".
    pub app_key: [u8; 32],
}

/// Walk the descendant tree of `signed_group_id` and evaluate the
/// `from_app_key == GroupMeta.app_key` predicate at each node, including
/// the signed group itself.
///
/// **Cycle and depth safety.** The walk maintains an explicit visited-set
/// and inserts each node into it *before* descending into its children,
/// so every node is processed at most once. That alone guarantees
/// termination on any finite graph — cyclic or not — because the stack
/// only ever grows by unvisited children, and the set of unvisited
/// children is monotonically shrinking. The production tree-shape
/// invariant is maintained by
/// [`nest_group`][crate::nest_group]'s pre-nest cycle
/// check (and the depth bound enforced there), so a real production
/// store never has a cycle to begin with — but if the store is
/// corrupted (or a future code path forgets the cycle check), this
/// function still terminates with a deduped result rather than spinning
/// forever, which is the property the cascade apply arm needs to
/// remain bounded under any store state.
///
/// Iteration order is depth-first by stable RocksDB key-byte order
/// within each parent's child list (matching
/// [`list_child_groups`][crate::list_child_groups]). The
/// caller does not depend on this order — both cascade apply arms iterate
/// the full result before issuing any writes — but it is deterministic
/// for debuggability.
pub fn walk_for_predicate(
    store: &Store,
    signed_group_id: ContextGroupId,
    from_app_key: [u8; 32],
) -> EyreResult<Vec<WalkEntry>> {
    let mut out: Vec<WalkEntry> = Vec::new();
    let mut visited: HashSet<ContextGroupId> = HashSet::new();
    let mut stack: Vec<ContextGroupId> = vec![signed_group_id];

    while let Some(current) = stack.pop() {
        if !visited.insert(current) {
            // Already emitted — skip the duplicate. Defensive: in a
            // well-formed tree the child-index is a forest, so a
            // descendant can't be reached twice. A duplicate here means
            // the store has a cycle or a diamond, and we don't want to
            // emit the same group twice (apply would attempt the
            // mutation twice and could see stale state on the second
            // pass).
            continue;
        }

        let meta_opt = MetaRepository::new(store).load(&current)?;
        let app_key = meta_opt.as_ref().map(|m| m.app_key).unwrap_or([0u8; 32]);
        let matched = app_key == from_app_key;
        out.push(WalkEntry {
            group_id: current,
            matched,
            app_key,
        });

        // Push children for further descent. `list_child_groups` returns
        // direct children only, in stable RocksDB key-byte order. We
        // skip already-visited children to keep `stack` size bounded
        // even when the store has a diamond / cycle (the visited check
        // at pop time would also catch this, but skipping here keeps
        // `stack` from blowing up to O(cycle-length²)).
        for child in NamespaceRepository::new(store).list_children(&current)? {
            if !visited.contains(&child) {
                stack.push(child);
            }
        }
    }

    Ok(out)
}
