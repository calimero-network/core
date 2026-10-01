//! `RootOp::AdminChanged` apply handler, reached through `RootOp::RootGuarded`. Extracted from
//! `NamespaceGovernance::execute_admin_changed` in #2481.

use super::context::NamespaceApplyCtx;
use crate::{
    MembershipError, MembershipPolicy, MembershipRepository, MetaRepository, NamespaceError,
};
use calimero_account::AccountId;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use eyre::{bail, Result as EyreResult};

/// Repoint the namespace's admin pin. Reached only through `RootGuarded`, whose
/// root proof has already been accepted for `signer_account`.
pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    new_admin: AccountId,
    signer_account: AccountId,
) -> EyreResult<()> {
    ctx.require_namespace_admin(&op.signer)?;
    let ns_gid = ContextGroupId::from(ctx.namespace_id().to_bytes());
    let store = ctx.store();

    // Owner-only. `meta.admin_identity` is the admin pin no member-row change can
    // revoke, so repointing it is taking the namespace: any admin could otherwise
    // name themselves or an accomplice and keep it for good.
    let owner = MetaRepository::new(store)
        .load(&ns_gid)?
        .ok_or(NamespaceError::RootMissing)?
        .owner_identity;
    if signer_account != owner {
        bail!(MembershipError::OnlyOwnerCanChangeAdmin(hex::encode(
            ns_gid.to_bytes()
        )));
    }

    // The incoming admin must already be a member of the namespace root.
    // Setting `admin_identity` to a non-member produces an admin with no
    // enumerable membership row — invisible to member listings and to any
    // path that derives authority from the membership set rather than the
    // meta field.
    let membership = MembershipRepository::new(store);
    let Some(existing_role) = membership.role_of(&ns_gid, &new_admin)? else {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(ns_gid.to_bytes()),
            identity: new_admin.to_string(),
        });
    };
    // An attested TEE is never made the namespace admin: that is the widest
    // form of moving a TEE row out of the TEE roles, which `MemberRoleSet` and
    // `MemberAdded` refuse too.
    MembershipPolicy::require_tee_row_keeps_tee_role(
        &new_admin,
        &existing_role,
        &GroupMemberRole::Admin,
    )?;

    let meta_repo = MetaRepository::new(store);
    let mut meta = meta_repo
        .load(&ns_gid)?
        .ok_or(NamespaceError::RootMissing)?;
    meta.admin_identity = new_admin;
    meta_repo.save(&ns_gid, &meta)?;

    // Ensure the new admin carries an explicit Admin member row so they are
    // enumerable as Admin AND so authority checks that read the membership-row
    // role (`MembershipRepository::is_admin`, reached via
    // `require_namespace_admin`) agree with `meta.admin_identity`. Upgrade ANY
    // non-Admin role left (a TEE role was refused above): Admin is the top
    // role, so this never downgrades, and leaving another role in place would
    // make `is_admin` return false for the very identity the meta names as
    // admin.
    if existing_role != GroupMemberRole::Admin {
        // Role-only update — `set_role` preserves the row's other fields rather
        // than zeroing them as a full-row `add_member` overwrite would.
        membership.set_role(&ns_gid, &new_admin, GroupMemberRole::Admin)?;
    }
    Ok(())
}
