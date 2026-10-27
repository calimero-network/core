//! `GroupOp::TransferOwnership` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MembershipError, MetaRepository};
use calimero_account::AccountId;
use calimero_primitives::context::GroupMemberRole;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, new_owner: &AccountId) -> EyreResult<()> {
    let _signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Owner-only — current owner is the only signer who can transfer.
    let mut meta = MetaRepository::new(store)
        .load(group_id)?
        .ok_or_else(|| MembershipError::UnknownGroup(hex::encode(group_id.to_bytes())))?;

    let ns_gid = crate::NamespaceRepository::new(store).resolve(group_id)?;
    let space_root = crate::private_space::Tree::at_cut(
        store,
        ctx.authorizer(),
        group_id,
        ctx.cut(),
        crate::TreeReads {
            chains: &[*group_id],
            subtrees: &[],
        },
    )?
    .space_of(group_id)?
        == Some(*group_id);
    if !ctx.signer_owns(space_root)? {
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
    // A space root's ownership is a board fact, decided where the space's rows cannot be read:
    // its heir need only stand in the workspace, and becomes the space's admin by the transfer.
    if space_root {
        if crate::PermissionChecker::new(store, ns_gid)
            .with_apply_auth(ctx.cut(), ctx.authorizer())
            .effective_role_at_cut(new_owner)?
            .is_none()
        {
            bail!(MembershipError::TransferTargetNotMember(hex::encode(
                group_id.to_bytes()
            )));
        }
        crate::MembershipRepository::new(store).add_member(
            group_id,
            new_owner,
            GroupMemberRole::Admin,
        )?;
    }
    if !space_root {
        match ctx.direct_role(new_owner)? {
            Some(GroupMemberRole::Admin) => {}
            Some(other) => bail!(MembershipError::TransferTargetNotAdmin {
                group: hex::encode(group_id.to_bytes()),
                role: other,
            }),
            None => bail!(MembershipError::TransferTargetNotMember(hex::encode(
                group_id.to_bytes()
            ))),
        }
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
