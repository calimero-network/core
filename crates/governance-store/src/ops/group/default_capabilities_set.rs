//! `GroupOp::DefaultCapabilitiesSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, capabilities: &u32) -> EyreResult<()> {
    let signer = ctx.signer();

    ctx.settings()
        .set_default_capabilities(signer, *capabilities)?;
    Ok(())
}
