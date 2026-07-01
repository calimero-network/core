//! `GroupOp::ContextCapabilityRevoked` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{get_group_for_context, CapabilitiesRepository, ContextRegistrationError};
use calimero_governance_types::ContextCapabilityBits;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    member: &PublicKey,
    capability: &ContextCapabilityBits,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions()
        .require_manage_members(signer, "revoke context capability")?;
    // Mirror the context↔group guard on the grant path: only touch a
    // per-context capability row for a context registered in this group.
    if get_group_for_context(store, context_id)? != Some(*group_id) {
        bail!(ContextRegistrationError::NotInGroup {
            group_id: hex::encode(group_id.to_bytes()),
            context_id: format!("{context_id:?}"),
        });
    }
    let caps = CapabilitiesRepository::new(store);
    let current = caps
        .context_member_capability(group_id, context_id, member)?
        .unwrap_or(0);
    let new_caps = current & !capability.get();
    if new_caps == 0 {
        caps.delete_context_member(group_id, context_id, member)?;
    } else {
        caps.set_context_member(group_id, context_id, member, new_caps)?;
    }
    Ok(())
}
