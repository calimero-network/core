//! `GroupOp::GroupDelete` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::group_store::{
    delete_group_local_rows, MembershipError, MetaError, MetaRepository, MetadataRepository,
};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Owner-only. Admins can no longer delete the group on their
    // own — only the owner can. Tightens the previous policy
    // (`require_admin`) which let any admin destroy the group.
    let meta = MetaRepository::new(store)
        .load(group_id)?
        .ok_or_else(|| MembershipError::UnknownGroup(hex::encode(group_id.to_bytes())))?;
    if meta.owner_identity != *signer {
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
