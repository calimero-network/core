//! `RootOp::GroupReparented` apply handler. Extracted from
//! `NamespaceGovernance::execute_group_reparented` in #2481.

use super::context::NamespaceApplyCtx;
use crate::op_events::OpEvent;
use crate::{NamespaceRepository, ReparentOutcome};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    child_group_id: [u8; 32],
    new_parent_id: [u8; 32],
) -> EyreResult<()> {
    ctx.require_namespace_admin(&op.signer)?;
    let child = ContextGroupId::from(child_group_id);
    let new_parent = ContextGroupId::from(new_parent_id);
    match NamespaceRepository::new(ctx.store()).reparent(&child, &new_parent)? {
        ReparentOutcome::Reparented { old_parent } => {
            let ns_id = ctx.namespace_id();
            ctx.queue_event(OpEvent::SubgroupReparented {
                namespace_id: ns_id,
                old_parent_group_id: old_parent.to_bytes(),
                new_parent_group_id: new_parent_id,
                child_group_id,
            });
        }
        // Idempotent no-op (new_parent == old_parent). Don't fire an
        // event — downstream subscribers would see a "reparent" with
        // identical old/new parents, falsely implying a structural
        // change occurred.
        ReparentOutcome::Unchanged => {}
    }
    Ok(())
}
