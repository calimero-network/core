//! `GroupOp::GroupDelete` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::private_space::Tree;
use crate::TreeReads;
use crate::{
    delete_group_and_parent_edge, MembershipError, MetaError, MetaRepository, NamespaceRepository,
};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    let _signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Owner-only. Admins can no longer delete the group on their
    // own — only the owner can. Tightens the previous policy
    // (`require_admin`) which let any admin destroy the group.
    if MetaRepository::new(store).load(group_id)?.is_none() {
        bail!(MembershipError::UnknownGroup(hex::encode(
            group_id.to_bytes()
        )));
    }
    let tree = Tree::at_cut(
        store,
        ctx.authorizer(),
        group_id,
        ctx.cut(),
        TreeReads {
            chains: &[*group_id],
            subtrees: &[*group_id],
        },
    )?;
    // Ownership is recorded against an account, so the signing key is resolved
    // before the comparison. An unresolvable key is not the owner.
    let space_root = tree.space_of(group_id)? == Some(*group_id);
    if !ctx.signer_owns(space_root)? {
        bail!(MembershipError::OnlyOwnerCanDelete(hex::encode(
            group_id.to_bytes()
        )));
    }
    // Owning an inner folder is no standing without a seat in its space, read at the cut; a
    // space root's owner needs none.
    if let (Some(space), Some(signer)) = (tree.space_of(group_id)?, ctx.signer_account()?) {
        if space != *group_id {
            let seated = match ctx
                .authorizer()
                .effective_role_at_cut(&space, &signer, ctx.cut())
            {
                Some(role) => role.is_some(),
                None => {
                    ctx.ensure_live_fallback_is_sound(ctx.signer())?;
                    crate::MembershipRepository::new(store).is_member(&space, &signer)?
                }
            };
            if !seated {
                bail!(MembershipError::OnlyOwnerCanDelete(hex::encode(
                    group_id.to_bytes()
                )));
            }
        }
    }
    tree.require_may_delete(group_id)?;
    // A context anywhere below would be left in a group nobody can reach, so the
    // whole subtree must be empty; its groups then go with it, children first.
    let subtree = NamespaceRepository::new(store).collect_subtree_for_cascade(group_id)?;
    if !subtree.contexts.is_empty() {
        bail!(MetaError::HasRegisteredContexts);
    }
    for group in subtree.descendant_groups.iter().chain([group_id]) {
        delete_group_and_parent_edge(store, group)?;
    }
    Ok(())
}
