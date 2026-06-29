//! `GroupOp::SubgroupVisibilitySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, mode: &u8) -> EyreResult<()> {
    let signer = ctx.signer();

    let visibility = match *mode {
        0 => calimero_context_config::VisibilityMode::Open,
        _ => calimero_context_config::VisibilityMode::Restricted,
    };
    ctx.settings().set_subgroup_visibility(signer, visibility)?;
    Ok(())
}
