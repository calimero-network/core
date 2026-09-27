//! `GroupOp::TeeVaultKeyDelivered` apply handler.
//!
//! Apply checks who published it: an admitted TEE, on the namespace root.
//! Whether the envelope opens, and to the key it names, only its recipient can
//! tell, so the recipient checks that when it reads it
//! (`crate::tee_vault::tee_vault_keys`).

use calimero_primitives::context::GroupMemberRole;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository, NamespaceRepository};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    // The reader resolves to the root, so a subgroup copy would be dead data.
    if NamespaceRepository::new(ctx.store())
        .parent(ctx.group_id())?
        .is_some()
    {
        bail!("TeeVaultKeyDelivered is namespace-scoped; it must be published on the root");
    }
    let Some(signer) = ctx.signer_account()? else {
        bail!(MembershipError::TeeVaultKeyNotFromTee);
    };
    if MembershipRepository::new(ctx.store()).role_of(ctx.group_id(), &signer)?
        != Some(GroupMemberRole::ReadOnlyTee)
    {
        bail!(MembershipError::TeeVaultKeyNotFromTee);
    }
    Ok(())
}
