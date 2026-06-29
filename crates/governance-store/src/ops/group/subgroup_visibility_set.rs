//! `GroupOp::SubgroupVisibilitySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use calimero_context_config::VisibilityMode;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, mode: &VisibilityMode) -> EyreResult<()> {
    let signer = ctx.signer();
    ctx.settings().set_subgroup_visibility(signer, *mode)?;
    Ok(())
}
