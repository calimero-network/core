//! `GroupOp::SubgroupVisibilitySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use calimero_context_config::VisibilityMode;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, mode: &VisibilityMode) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    ctx.settings().set_subgroup_visibility(signer, *mode)?;
    // Re-trigger inherited auto-follow for this subgroup's contexts. A
    // root-admitted member (e.g. a `ReadOnlyTee`) inherits membership only into
    // `Open` subgroups; a flip that applies after the contexts were registered
    // (e.g. a `SubgroupVisibilitySet -> Open` re-driven late, once the namespace
    // key arrived) would otherwise never re-run the follow decision. See
    // `OpEvent::SubgroupVisibilityChanged` and `calimero_context::auto_follow`.
    ctx.queue_event(crate::op_events::OpEvent::SubgroupVisibilityChanged {
        group_id: group_id.to_bytes(),
        open: matches!(mode, VisibilityMode::Open),
    });
    Ok(())
}
