//! `GroupOp::MemberSetAutoFollow` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository};
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    target: &PublicKey,
    auto_follow_contexts: &bool,
    auto_follow_subgroups: &bool,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Admin-or-self: admin can toggle flags for any member, a
    // member can toggle their own. Non-admin, non-self attempts
    // are rejected.
    if !ctx.permissions().is_admin(signer)? && signer != target {
        bail!(MembershipError::AutoFollowAuthFailed);
    }
    // Target must already be a group member.
    if MembershipRepository::new(store)
        .role_of(group_id, target)?
        .is_none()
    {
        bail!(MembershipError::NotMember {
            group_id: format!("{group_id:?}"),
            identity: format!("{target:?}"),
        });
    }
    let flags = calimero_store::key::AutoFollowFlags {
        contexts: *auto_follow_contexts,
        subgroups: *auto_follow_subgroups,
    };
    MembershipRepository::new(store).set_auto_follow(group_id, target, flags)?;
    ctx.queue_event(crate::op_events::OpEvent::AutoFollowSet {
        group_id: group_id.to_bytes(),
        member: *target,
        contexts: *auto_follow_contexts,
        subgroups: *auto_follow_subgroups,
    });
    Ok(())
}
