//! `GroupOp::ContextCapabilityGranted` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::context_registration::registered_at_cut;
use crate::{CapabilitiesRepository, ContextRegistrationError, MembershipError};
use calimero_account::AccountId;
use calimero_governance_types::ContextCapabilityBits;
use calimero_primitives::context::ContextId;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    member: &AccountId,
    capability: &ContextCapabilityBits,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions()
        .require_manage_members(signer, "grant context capability")?;
    // The context must be registered in THIS group at the op's cut, or the grant
    // writes a capability row scoped to a context another group owns. A detach
    // concurrent with it leaves the row behind on every node alike.
    if !registered_at_cut(store, group_id, context_id, ctx.cut(), ctx.authorizer())? {
        bail!(ContextRegistrationError::NotInGroup {
            group_id: hex::encode(group_id.to_bytes()),
            context_id: context_id.to_string(),
        });
    }
    // The grantee must be a DIRECT member of this group (a `GroupMember` row),
    // mirroring `MemberCapabilitySet`. Without this a `manage_members` signer
    // could write a per-context capability row for an arbitrary non-member
    // identity — an orphan row the enumeration/authorization paths never
    // reconcile. `role_of` is deliberately direct-only: an INHERITED member
    // (e.g. a parent-group admin who never joined this subgroup) is rejected
    // here and must be added as a direct member before receiving a per-context
    // grant.
    if ctx.direct_role(member)?.is_none() {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(group_id.to_bytes()),
            identity: member.to_string(),
        });
    }
    let caps = CapabilitiesRepository::new(store);
    let current = caps
        .context_member_capability(group_id, context_id, member)?
        .unwrap_or(0);
    caps.set_context_member(group_id, context_id, member, current | capability.get())?;
    Ok(())
}
