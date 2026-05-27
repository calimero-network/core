//! `GroupOp::ContextDetached` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use calimero_primitives::context::ContextId;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, context_id: &ContextId) -> EyreResult<()> {
    let signer = ctx.signer();
    let permissions = ctx.permissions();
    ctx.context_registration()
        .detach(permissions, signer, context_id)?;
    Ok(())
}
