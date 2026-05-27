//! `GroupOp::TargetApplicationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use calimero_primitives::application::ApplicationId;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
) -> EyreResult<()> {
    let signer = ctx.signer();

    ctx.settings()
        .set_target_application(signer, app_key, target_application_id)?;
    Ok(())
}
