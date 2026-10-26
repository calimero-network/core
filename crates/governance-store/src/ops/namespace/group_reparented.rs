//! `RootOp::GroupReparented` apply handler. Extracted from
//! `NamespaceGovernance::execute_group_reparented` in #2481.

use super::context::NamespaceApplyCtx;
use crate::op_events::OpEvent;
use crate::private_space::Tree;
use crate::TreeReads;
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
    let child = ContextGroupId::from(child_group_id);
    let new_parent = ContextGroupId::from(new_parent_id);
    // `reparent` keeps child and new parent in one namespace; this makes it the op's.
    ctx.require_in_namespace(&child)?;
    let (parents, authorizer) = ctx.apply_auth();
    let ns_gid = ContextGroupId::from(ctx.namespace_id().to_bytes());
    let tree = Tree::at_cut(
        ctx.store(),
        authorizer,
        &ns_gid,
        parents,
        TreeReads {
            chains: &[child, new_parent],
            subtrees: &[child],
        },
    )?;
    ctx.require_tree_authority(&op.signer, &tree, &[child, new_parent])?;
    tree.require_may_reparent(&child, &new_parent)?;
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
