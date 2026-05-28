//! `GroupOp::TransferOwnership` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository, MetaRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, new_owner: &PublicKey) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Owner-only — current owner is the only signer who can transfer.
    let mut meta = MetaRepository::new(store)
        .load(group_id)?
        .ok_or_else(|| MembershipError::UnknownGroup(hex::encode(group_id.to_bytes())))?;

    if meta.owner_identity != *signer {
        bail!(MembershipError::OnlyOwnerCanTransfer(hex::encode(
            group_id.to_bytes()
        )));
    }

    // The new owner must already be an Admin of the group. Transfer
    // does not implicitly invite or promote — the successor must
    // already be in place at admin tier. This prevents two awkward
    // states:
    //   * Transferring to a non-member: would create an absentee
    //     owner.
    //   * Transferring to a plain Member: Owner has all Admin
    //     privileges by design (see doc § 7 privilege matrix), so
    //     a plain-Member owner would have a confusing "owner with
    //     reduced capabilities" status. Require Admin first;
    //     promote then transfer if needed.
    match MembershipRepository::new(store).role_of(group_id, new_owner)? {
        Some(GroupMemberRole::Admin) => {}
        Some(other) => bail!(
            "new owner of group {} must be an Admin, but is currently {:?}; \
             promote them to Admin before transferring ownership",
            hex::encode(group_id.to_bytes()),
            other
        ),
        None => bail!(
            "new owner is not a member of group {}; invite and promote them first",
            hex::encode(group_id.to_bytes())
        ),
    }

    meta.owner_identity = *new_owner;
    MetaRepository::new(store).save(group_id, &meta)?;
    Ok(())
}
