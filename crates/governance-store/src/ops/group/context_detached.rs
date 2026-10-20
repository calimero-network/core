//! `GroupOp::ContextDetached` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::context_registration::registered_at_cut;
use calimero_primitives::context::ContextId;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, context_id: &ContextId) -> EyreResult<()> {
    super::shared_writers_rotated::refuse_moving_rotated_context(ctx, context_id, None)?;
    let signer = ctx.signer();
    let registered = registered_at_cut(
        ctx.store(),
        ctx.group_id(),
        context_id,
        ctx.cut(),
        ctx.authorizer(),
    )?;
    let permissions = ctx.permissions();
    ctx.context_registration()
        .detach(permissions, signer, context_id, registered)?;
    Ok(())
}
