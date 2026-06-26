//! `GroupOp::ContextCapabilityGranted` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::CapabilitiesRepository;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    member: &PublicKey,
    capability: &u8,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions()
        .require_manage_members(signer, "grant context capability")?;
    let caps = CapabilitiesRepository::new(store);
    let current = caps
        .context_member_capability(group_id, context_id, member)?
        .unwrap_or(0);
    caps.set_context_member(group_id, context_id, member, current | capability)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::GroupApplyCtx;
    use crate::test_fixtures::{test_group_id, test_store};
    use crate::{MembershipRepository, LIVE_FALLBACK_AUTHORIZER};
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::PublicKey;

    #[test]
    fn non_admin_without_manage_members_cannot_grant_capability() {
        let store = test_store();
        let gid = test_group_id();
        let signer = PublicKey::from([0x02; 32]); // plain Member, no MANAGE_MEMBERS cap
        let target = PublicKey::from([0x03; 32]);
        let context_id = ContextId::from([0x04; 32]);
        let capability: u8 = 0b0000_0001;

        MembershipRepository::new(&store)
            .add_member(&gid, &signer, GroupMemberRole::Member)
            .unwrap();

        let mut ctx = GroupApplyCtx::new_with_apply_auth(
            &store,
            &gid,
            &signer,
            &[],
            &LIVE_FALLBACK_AUTHORIZER,
        );
        assert!(
            super::apply(&mut ctx, &context_id, &target, &capability).is_err(),
            "plain Member without MANAGE_MEMBERS should be rejected"
        );
    }
}
