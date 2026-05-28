//! `GroupOp::Noop` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use eyre::Result as EyreResult;

pub(crate) fn apply(_ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    Ok(())
}
