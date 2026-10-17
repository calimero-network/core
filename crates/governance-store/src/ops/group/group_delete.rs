//! `GroupOp::GroupDelete` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{
    delete_group_local_rows, MembershipError, MetaError, MetaRepository, MetadataRepository,
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
    if MetadataRepository::new(store).count_contexts(group_id)? > 0 {
        bail!(MetaError::HasRegisteredContexts);
    }
    delete_group_local_rows(store, group_id)?;
    Ok(())
}
