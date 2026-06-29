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
        Some(other) => bail!(MembershipError::TransferTargetNotAdmin {
            group: hex::encode(group_id.to_bytes()),
            role: other,
        }),
        None => bail!(MembershipError::TransferTargetNotMember(hex::encode(
            group_id.to_bytes()
        ))),
    }

    // Move BOTH the owner pin and the meta admin pin to the successor.
    //
    // `is_admin` treats `meta.admin_identity` as an always-admin that no
    // member-row change can revoke (see `MembershipRepository::is_admin`).
    // At group/namespace genesis the creator is written as
    // `admin_identity == owner_identity`, so leaving `admin_identity` on
    // the old owner here would let a former owner keep permanent,
    // unrevokable admin after handing ownership over. Re-pinning it to the
    // new owner preserves the genesis invariant (owner is the meta admin)
    // and demotes the former owner to whatever revokable member row they
    // still hold (an explicit Admin row, if any — which can now be removed
    // or demoted like any other).
    meta.owner_identity = *new_owner;
    meta.admin_identity = *new_owner;
    MetaRepository::new(store).save(group_id, &meta)?;
    Ok(())
}
