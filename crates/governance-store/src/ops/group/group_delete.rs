//! `GroupOp::GroupDelete` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
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
    // Ownership is recorded against an account, so the signing key is resolved
    // before the comparison. An unresolvable key is not the owner.
    let owner = crate::group_owner::owner_at_cut(store, group_id, ctx.cut(), ctx.authorizer())?;
    if ctx
        .signer_account()?
        .is_none_or(|signer| Some(signer) != owner)
    {
        bail!(MembershipError::OnlyOwnerCanDelete(hex::encode(
            group_id.to_bytes()
        )));
    }
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
