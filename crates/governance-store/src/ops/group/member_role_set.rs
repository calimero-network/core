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
    // A role change targets an EXISTING direct member. `add_member` is an
    // unconditional upsert, so without this guard a `MemberRoleSet` on a
    // non-member (or on a previously-removed member still on the deny-list)
    // silently CREATES a member row — bypassing the `MemberAdded` path that
    // clears the deny-list and restores per-context identities. Require a
    // direct membership row so this op only ever mutates the role of someone
    // who is already a member.
    let membership = MembershipRepository::new(store);
    if membership.role_of(group_id, member)?.is_none() {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(group_id.to_bytes()),
            identity: format!("{member:?}"),
        });
    }
    ctx.membership_policy()
        .ensure_not_last_admin_demotion(member, role)?;
    // Role-only update: preserve the member row's other fields
    // (`private_key`/`sender_key`/`auto_follow`) instead of the full-row
    // overwrite `add_member` would do.
    membership.set_role(group_id, member, role.clone())?;
    Ok(())
}
