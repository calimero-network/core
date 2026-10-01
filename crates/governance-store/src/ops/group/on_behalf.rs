//! `GroupOp::OnBehalf` apply handler: a member's group op, published by a relay.
//!
//! The wrapper is admitted by [`crate::delegation_gate`]; the inner op is then
//! applied through the ordinary dispatcher, at the same cut, with the author as
//! the acting principal — so it meets exactly the gate it would have met had the
//! member signed it themselves.

use calimero_account::GovernanceDelegation;
use calimero_context_client::local_governance::GroupOp;
use eyre::Result as EyreResult;

use super::context::GroupApplyCtx;
use super::dispatch;
use crate::delegation_gate::{check_group_delegation, spend_delegation_nonce};
use crate::first_target_gate::refuse_unless_first_target;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    inner: &GroupOp,
    delegation: &GovernanceDelegation,
) -> EyreResult<()> {
    let store = ctx.store();
    let group_id = ctx.group_id();
    let (warrant, principal) = check_group_delegation(
        store,
        ctx.permissions(),
        group_id,
        ctx.signer(),
        inner,
        delegation,
    )?;
    // A relay carries a group's first application choice, never an upgrade,
    // and whether it is the first is read at this op's cut.
    if matches!(inner, GroupOp::TargetApplicationSet { .. }) {
        refuse_unless_first_target(store, group_id, ctx.cut(), ctx.authorizer())?;
    }

    let author_key = principal.key;
    let mut inner_ctx = GroupApplyCtx::new_as(
        store,
        group_id,
        &author_key,
        ctx.cut(),
        ctx.authorizer(),
        Some(principal),
    );
    let handled = dispatch(&mut inner_ctx, inner)?;
    if !handled {
        eyre::bail!("the delegated {} op has no handler", inner.op_kind_label());
    }
    spend_delegation_nonce(store, group_id, &warrant)?;

    if inner_ctx.divergence.is_some() {
        ctx.divergence = inner_ctx.divergence.take();
    }
    for event in std::mem::take(&mut inner_ctx.pending_events) {
        ctx.queue_event(event);
    }
    Ok(())
}
