//! `GroupOp::UpgradePolicySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use calimero_primitives::context::UpgradePolicy;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, policy: &UpgradePolicy) -> EyreResult<()> {
    let signer = ctx.signer();

    ctx.settings().set_upgrade_policy(signer, policy)?;
    Ok(())
}
