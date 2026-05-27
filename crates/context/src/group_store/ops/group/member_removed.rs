//! `GroupOp::MemberRemoved` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::verify_post_apply_state_hashes;
use super::context::GroupApplyCtx;
use crate::group_store::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipError,
    MembershipRepository, MetaRepository,
};
use calimero_primitives::context::ContextId;
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
    crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    Ok(())
}
