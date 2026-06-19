//! `GroupOp::MemberRemoved` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::verify_post_apply_state_hashes;
use super::context::GroupApplyCtx;
use crate::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipError,
    MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    expected_group_state_hash: &[u8; 32],
    expected_context_state_hashes: &[(ContextId, [u8; 32])],
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions()
        .require_manage_members(signer, "remove member")?;
    ctx.permissions()
        .require_admin_to_remove_admin(signer, member)?;
    // Owner is immune to involuntary removal. Owner must
    // `TransferOwnership` first to step down, then they can be
    // removed (or self-leave once that op exists).
    if let Some(meta) = MetaRepository::new(store).load(group_id)? {
        if meta.owner_identity == *member {
            bail!(MembershipError::OwnerImmuneFromRemoval(hex::encode(
                group_id.to_bytes()
            )));
        }
    }
    ctx.membership_policy()
        .ensure_not_last_admin_removal(member)?;
    // Capture role BEFORE removal so we can emit a role-scoped
    // `TeeMemberRemoved` event alongside the generic `MemberRemoved`.
    // The cascade below touches `ContextIdentity` rows only (disjoint
    // column from `GroupMember`), and `remove_member` is the call that
    // clears the role row — so `role_of` is still authoritative here.
    //
    // `role_of` legitimately returns `None` here, so this must NOT bail:
    // a member can be removed from an Open subgroup where their membership
    // is *inherited* from a parent group and they hold no direct
    // `GroupMember` row (exercised by the `group-kick-and-rejoin-keyshare`
    // and `group-kick-and-readd-deny-list` e2e flows). The preceding checks
    // do not guarantee a direct row — `require_manage_members` gates the
    // *signer*, and `ensure_not_last_admin_removal` short-circuits for any
    // non-admin (which includes a member with no row at all). So `None` is
    // the no-direct-row case, not an inconsistency: such a member cannot
    // have held `ReadOnlyTee` (a directly-rowed role), so skipping the
    // `TeeMemberRemoved` follow-up is exactly right, while the generic
    // `MemberRemoved` + deny-list below still fire to drive the soft-leave
    // path. Log at `debug!`, not `warn!`: this is an expected, common path
    // for Open subgroups (inherited-member removals), not an anomaly —
    // warning on every one would be pure noise.
    let removed_role = MembershipRepository::new(store).role_of(group_id, member)?;
    if removed_role.is_none() {
        tracing::debug!(
            group_id = %hex::encode(group_id.to_bytes()),
            member = %member,
            "MemberRemoved apply: role_of returned None (no direct row — likely \
             inherited membership); skipping TeeMemberRemoved follow-up"
        );
    }
    // A namespace-root removal of a `ReadOnlyTee` evicts it namespace-wide:
    // the TEE's presence in any subgroup came from namespace-level
    // attestation policy (`tee_subgroup_admit`), not the subgroup admin's
    // choice, so root authority extends to it. Cascade per-receiver like a
    // self-`MemberLeft` namespace-leave; scoped to `ReadOnlyTee` so
    // normal-member Restricted-subgroup membership autonomy (#2256) is
    // untouched. The per-subgroup cascade events are queued BEFORE the root
    // events (below), but note this ordering is NOT a store-state causality:
    // every event rides the emit-after-persist sink and is delivered only
    // AFTER this apply returns and all mutations (root row included) are
    // persisted. So a subscriber processing the subgroup `TeeMemberRemoved`
    // already observes the root row removed; the queue order merely sequences
    // the order subscribers see the events, not the state they read. Mirrors
    // the `is_namespace_leave` block in `member_left.rs`, but DELIBERATELY
    // omits the owner-self and per-descendant last-admin checks: a
    // `ReadOnlyTee` is structurally never an owner or admin, so both are inert
    // here and would mislead.
    //
    // Gate the namespace-root resolution behind the already-loaded role
    // check: `resolve` walks the parent chain (O(depth)) and only a
    // `ReadOnlyTee` removal cascades, so every non-TEE removal skips the walk.
    if removed_role == Some(GroupMemberRole::ReadOnlyTee) {
        let namespaces = NamespaceRepository::new(store);
        // Relies on `resolve` being reflexive for roots: `resolve(root) ==
        // root`. So equality here means `group_id` IS the namespace root (a
        // subgroup-level TEE removal resolves to a different root and skips the
        // cascade).
        let is_namespace_root = namespaces.resolve(group_id)? == *group_id;
        if is_namespace_root {
            let membership = MembershipRepository::new(store);
            let deny_list = DenyListRepository::new(store);
            // `collect_descendants` returns the FULL subtree (every level, not
            // just direct children) and excludes the root itself, so one pass
            // covers each descendant group's own directly-registered contexts.
            let descendants = namespaces.collect_descendants(group_id)?;
            for sub in &descendants {
                // `ContextIdentity` hygiene runs for EVERY descendant —
                // INCLUDING Open subgroups the TEE only *inherited* into (no
                // direct `GroupMember` row) yet still auto-followed contexts of
                // (Fix B). Skipping those would strand the evicted node's
                // `ContextIdentity` rows there. `cascade_remove_member` is an
                // idempotent no-op where the TEE holds no rows and touches only
                // `ContextIdentity` (disjoint from `GroupMember`), so it is
                // group-state-hash-neutral.
                cascade_remove_member_from_group_tree(store, sub, member)?;
                // Membership teardown + role-scoped events fire only where the
                // TEE holds a DIRECT row — inherited rows have no per-subgroup
                // `GroupMember` to evict and never emitted a join event. The
                // cascade ENTRY is gated on the ROOT role (the security
                // boundary); this per-descendant role gate is only for the
                // event split.
                let Some(role) = membership.role_of(sub, member)? else {
                    continue;
                };
                membership.remove_member(sub, member)?;
                // Deny-listing is intentionally gated to DIRECT rows (mirrors
                // `member_left.rs`). A deny-list entry drops the member's
                // state-delta traffic on the group's topics "until they
                // re-join", and is cleared by a direct re-add. Inherited Open
                // subgroups have no direct row and no subgroup-level re-join op
                // to clear it: re-inheritance is re-evaluated from the root
                // row. Marking them here would strand a stale entry that
                // wrongly drops the traffic of a legitimately re-inherited node
                // after a root re-admission. The `ContextIdentity` purge above
                // is safe to run namespace-wide precisely because it is
                // re-created on the next join/auto-follow; the deny-list is not.
                deny_list.mark(sub, member)?;
                ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
                    group_id: sub.to_bytes(),
                    member: *member,
                });
                if role == GroupMemberRole::ReadOnlyTee {
                    ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
                        group_id: sub.to_bytes(),
                        member: *member,
                    });
                }
            }
        }
    }
    cascade_remove_member_from_group_tree(store, group_id, member)?;
    MembershipRepository::new(store).remove_member(group_id, member)?;
    // Add to deny-list: state deltas from this member will be
    // dropped at the receive entry point before the cross-DAG
    // check runs. Cleared if/when the member is re-added.
    DenyListRepository::new(store).mark(group_id, member)?;
    // Ordering invariant: `verify_post_apply_state_hashes`
    // must run AFTER the last mutation that touches inputs
    // to `compute_group_state_hash` (i.e. `GroupMeta` rows
    // and `GroupMember` rows for this `group_id`). Of the
    // three preceding steps here only `remove_group_member`
    // touches those inputs:
    //
    // * `cascade_remove_member_from_group_tree` deletes
    //   `ContextIdentity` rows in the state-DAG-layer
    //   column — disjoint from `GroupMember`. Does not
    //   affect the hash.
    // * `mark_denied` writes a `GroupDeniedMember` row — a
    //   separate column. Does not affect the hash.
    // * `remove_group_member` deletes the `GroupMember`
    //   row — this is the step the pre-apply simulation
    //   in `compute_group_state_hash_after_remove` mirrors.
    //
    // Adding any future mutation between
    // `remove_group_member` and this call that DOES touch
    // `GroupMeta` or `GroupMember` rows for `group_id` will
    // make the recomputed hash diverge from the signed
    // claim on every honest receiver. The pre-apply
    // simulation only models the single removal; any extra
    // mutation here is invisible to it.
    ctx.divergence = verify_post_apply_state_hashes(
        store,
        group_id,
        "MemberRemoved",
        expected_group_state_hash,
        expected_context_state_hashes,
    );
    ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    // Role-scoped follow-up: TEE evictions need extra local hygiene
    // (forward-secrecy purge in `calimero_context::self_purge`) that the
    // soft-leave path deliberately skips. Non-TEE removals stay
    // soft-leave so rejoin/keyshare flows can re-use the local rows.
    if removed_role == Some(GroupMemberRole::ReadOnlyTee) {
        ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
            group_id: group_id.to_bytes(),
            member: *member,
        });
    }
    Ok(())
}
