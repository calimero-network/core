//! `GroupOp::MemberCapabilitySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::CapabilitiesRepository;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    capabilities: &u32,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_admin(signer)?;
    CapabilitiesRepository::new(store).set_member_capability(group_id, member, *capabilities)?;
    Ok(())
}
