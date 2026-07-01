//! `RootOp::AdminChanged` apply handler. Extracted from
//! `NamespaceGovernance::execute_admin_changed` in #2481.

use super::context::NamespaceApplyCtx;
use crate::{MembershipError, MembershipRepository, MetaRepository, NamespaceError};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    new_admin: PublicKey,
) -> EyreResult<()> {
    ctx.require_namespace_admin(&op.signer)?;
    let ns_gid = ContextGroupId::from(ctx.namespace_id().to_bytes());
    let store = ctx.store();

    // The incoming admin must already be a member of the namespace root.
    // Setting `admin_identity` to a non-member produces an admin with no
    // enumerable membership row — invisible to member listings and to any
    // path that derives authority from the membership set rather than the
    // meta field.
    let membership = MembershipRepository::new(store);
    let existing_role = membership.role_of(&ns_gid, &new_admin)?;
    if existing_role.is_none() {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(ns_gid.to_bytes()),
            identity: format!("{new_admin:?}"),
        });
    }

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
    // non-Admin role: Admin is the top role, so this never downgrades, and
    // leaving a `ReadOnlyTee` (or any future non-Admin role) in place would make
    // `is_admin` return false for the very identity the meta names as admin.
    // (`existing_role` is `Some` here — the `None` case bailed above.)
    if !matches!(existing_role, Some(GroupMemberRole::Admin)) {
        // Role-only update — `set_role` preserves the row's other fields rather
        // than zeroing them as a full-row `add_member` overwrite would.
        membership.set_role(&ns_gid, &new_admin, GroupMemberRole::Admin)?;
    }
    Ok(())
}
