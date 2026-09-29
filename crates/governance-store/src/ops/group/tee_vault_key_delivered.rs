//! `GroupOp::TeeVaultKeyDelivered` apply handler.
//!
//! Apply checks who published it: on the namespace root, an admitted TEE with
//! logged evidence for its signing key.
//! Whether the envelope opens, and to the key it names, only its recipient can
//! tell, so the recipient checks that when it reads it
//! (`crate::tee_vault::tee_vault_keys`).

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
    if !MembershipRepository::new(ctx.store())
        .role_of(ctx.group_id(), &signer)?
        .is_some_and(|role| role.is_tee())
    {
        bail!(MembershipError::TeeVaultKeyNotFromTee);
    }
    // A TEE row is not enough: the evidence must bind the signing key under an
    // image the authoring policy names.
    if !crate::tee::logged_evidence_admits_key(ctx.store(), ctx.group_id(), &signer, ctx.signer())?
    {
        bail!(MembershipError::TeeVaultKeyNotFromTee);
    }
    Ok(())
}
