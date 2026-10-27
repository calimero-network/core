//! `GroupOp::MemberRemoved` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::verify_post_apply_state_hashes;
use super::context::GroupApplyCtx;
use crate::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipError,
    MembershipRepository, NamespaceRepository, ReentryRepository,
};
use calimero_account::AccountId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_store::key::GroupExitReason;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &AccountId,
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
    if crate::group_owner::owner_at_cut(store, group_id, ctx.cut(), ctx.authorizer())?
        == Some(*member)
    {
        bail!(MembershipError::OwnerImmuneFromRemoval(hex::encode(
            group_id.to_bytes()
        )));
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
    // have held a TEE role (a directly-rowed role), so skipping the
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
    // A root (no parent edge) removal reaches every descendant row, for any role.
    if NamespaceRepository::new(store).parent(group_id)?.is_none() {
        super::namespace_departure::cascade_to_descendants(
            ctx,
            group_id,
            member,
            GroupExitReason::Removed,
        )?;
    }
    cascade_remove_member_from_group_tree(store, group_id, member)?;
    MembershipRepository::new(store).remove_member(group_id, member)?;
    // Add to deny-list: state deltas from this member will be
    // dropped at the receive entry point before the cross-DAG
    // check runs. Cleared if/when the member is re-added.
    DenyListRepository::new(store).mark(group_id, member)?;
    // If this is a namespace-root removal, the member also loses every
    // *inherited* Open-subgroup membership that flowed from this root row.
    // Record a namespace-root-keyed inherited-deny so the receive filter
    // fast-drops their deltas to those descendant subgroups too — the direct
    // deny-list above only covers this exact group. Cleared on root re-admission
    // (`add_member_with_keys`), which necessarily precedes any re-inheritance.
    if NamespaceRepository::new(store).parent(group_id)?.is_none() {
        DenyListRepository::new(store).mark_inherited(group_id, member)?;
    }
    // Block re-entry. Distinct from the deny-list above in both purpose and
    // lifetime: the deny-list silences their traffic and is retracted the moment
    // any member row is written, while this block is authorization state that
    // deliberately SURVIVES a member-row write so a re-join attempt fails. Only
    // an admin `MemberAdded` lifts a `Removed` block — no invitation does,
    // however freshly issued, or a kick from a group with a live open invitation
    // would mean nothing.
    ReentryRepository::new(store).block(group_id, member, GroupExitReason::Removed)?;
    // Ordering invariant: `verify_post_apply_state_hashes`
    // must run AFTER the last mutation that touches inputs
    // to `compute_group_state_hash` (i.e. `GroupMeta` rows
    // and `GroupMember` rows for this `group_id`). Of the
    // four preceding steps here only `remove_group_member`
    // touches those inputs:
    //
    // * `cascade_remove_member_from_group_tree` deletes
    //   `ContextIdentity` rows in the state-DAG-layer
    //   column — disjoint from `GroupMember`. Does not
    //   affect the hash.
    // * `mark_denied` writes a `GroupDeniedMember` row — a
    //   separate column. Does not affect the hash.
    // * the re-entry `block` writes a `GroupReentryBlock`
    //   row — also a separate column, and deliberately kept
    //   out of the hash. Were it hashed, the sign-time
    //   simulation in `compute_state_hash_after_remove`
    //   would have to model this write too, and every
    //   honest removal would otherwise report a false
    //   divergence and trigger a state re-fetch.
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
    //
    // The descendant cascade above is hash-neutral for this check: it touches only
    // other groups' rows, and its loop skips the root itself.
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
    if removed_role.as_ref().is_some_and(GroupMemberRole::is_tee) {
        ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
            group_id: group_id.to_bytes(),
            member: *member,
        });
    }
    Ok(())
}
