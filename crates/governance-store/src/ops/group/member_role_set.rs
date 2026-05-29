//! `GroupOp::MemberRoleSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    if *role == GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::ReadOnlyTeeViaAttestationOnly);
    }
    ctx.permissions().require_admin(signer)?;
    ctx.membership_policy()
        .ensure_not_last_admin_demotion(member, role)?;
    MembershipRepository::new(store).add_member(group_id, member, role.clone())?;
    Ok(())
}
