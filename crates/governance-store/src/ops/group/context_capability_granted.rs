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
