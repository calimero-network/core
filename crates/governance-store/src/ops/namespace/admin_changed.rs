//! `RootOp::AdminChanged` apply handler. Extracted from
//! `NamespaceGovernance::execute_admin_changed` in #2481.

use super::context::NamespaceApplyCtx;
use crate::{MetaRepository, NamespaceError};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    new_admin: PublicKey,
) -> EyreResult<()> {
    ctx.require_namespace_admin(&op.signer)?;
    let ns_gid = ContextGroupId::from(ctx.namespace_id());
    let meta_repo = MetaRepository::new(ctx.store());
    let mut meta = meta_repo
        .load(&ns_gid)?
        .ok_or(NamespaceError::RootMissing)?;
    meta.admin_identity = new_admin;
    meta_repo.save(&ns_gid, &meta)?;
    Ok(())
}
