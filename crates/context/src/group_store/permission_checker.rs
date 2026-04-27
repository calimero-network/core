use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::membership::{is_group_admin_or_has_capability, is_inherited_admin};
use super::GroupStoreError;

/// Authorization service for group governance operations.
///
/// This object centralizes permission checks so callers can express intent
/// (`require_manage_members`, `require_can_create_context`) instead of wiring
/// capability bits and error messages at each callsite.
pub struct PermissionChecker<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> PermissionChecker<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        // Issue #2256: admin authority cascades into Open subgroups
        // from any ancestor where the signer is a direct admin.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path` because the latter
        // short-circuits to `Direct` as soon as the identity has any
        // direct membership row in the target subgroup — even a
        // non-admin `Member` row — which would suppress inherited
        // admin authority for parent admins who happen to also be
        // explicit subgroup members.
        is_inherited_admin(self.store, &self.group_id, identity)
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_admin(identity)? {
            return Ok(());
        }
        // `is_admin` (via `is_inherited_admin`) is a strict superset of
        // the direct admin check, including the `GroupMeta.admin_identity`
        // fallback. Falling through to `membership.require_admin` here
        // would just re-run `is_group_admin` to format an error. Bail
        // directly with the same shape `require_group_admin` uses, so
        // callers that match on `GroupStoreError::NotAdmin` keep working.
        bail!(GroupStoreError::NotAdmin {
            group_id: format!("{:?}", self.group_id),
            identity: format!("{:?}", identity),
        });
    }

    pub fn require_manage_members(&self, identity: &PublicKey, operation: &str) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_MEMBERS)? {
            return Ok(());
        }
        // `is_authorized_with_capability` is a strict superset of the
        // direct admin-or-cap check, so falling through to
        // `require_group_admin_or_capability` would just redo the same
        // store reads to format an error. Bail directly with the same
        // diagnostic shape.
        bail!(GroupStoreError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: operation.to_owned(),
        });
    }

    pub fn require_manage_application(
        &self,
        identity: &PublicKey,
        operation: &str,
    ) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_APPLICATION)? {
            return Ok(());
        }
        bail!(GroupStoreError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: operation.to_owned(),
        });
    }

    pub fn require_can_create_context(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::CAN_CREATE_CONTEXT)? {
            return Ok(());
        }

        bail!("only group admin or members with CAN_CREATE_CONTEXT can register a context")
    }

    /// Resolves "admin or holds `capability_bit`" with Open-subgroup
    /// inheritance applied (issue #2256).
    ///
    /// Direct authority in `self.group_id` short-circuits. Otherwise:
    ///
    /// - **Admins** at any ancestor in the Open chain inherit governance
    ///   authority unconditionally (mirrors the structural-inheritance
    ///   model for parent admins).
    /// - **Non-admin** inherited members do **not** inherit governance
    ///   capabilities (`MANAGE_MEMBERS`, `MANAGE_APPLICATION`,
    ///   `CAN_CREATE_CONTEXT`, `CAN_INVITE_MEMBERS`, etc.). Their
    ///   cross-boundary authority is scoped to *context join/read* via
    ///   `CAN_JOIN_OPEN_SUBGROUPS` — the bit that already gated their
    ///   passing the membership walk in
    ///   [`super::membership::check_group_membership_path`]. Inheriting
    ///   arbitrary parent-level capabilities into the subgroup would be
    ///   a privilege-escalation path: a parent member with
    ///   `MANAGE_MEMBERS` at the namespace could otherwise add/remove
    ///   members in every Open subgroup, even though the subgroup admin
    ///   may not have intended to delegate that authority.
    ///
    /// Subgroup admins must grant governance capabilities explicitly at
    /// the subgroup level for non-admin parent members.
    fn is_authorized_with_capability(
        &self,
        identity: &PublicKey,
        capability_bit: u32,
    ) -> EyreResult<bool> {
        if is_group_admin_or_has_capability(self.store, &self.group_id, identity, capability_bit)? {
            return Ok(true);
        }
        // Only admin-inherited authority crosses the parent boundary;
        // non-admin caps must be explicit at the subgroup level.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path`'s `Inherited{via_admin:true}`
        // branch — the path walker short-circuits to `Direct` as soon
        // as any direct membership row exists in the target subgroup,
        // which would mask inherited admin authority for a parent
        // admin who is also an explicit non-admin subgroup member.
        is_inherited_admin(self.store, &self.group_id, identity)
    }

    pub fn require_admin_to_add_admin(
        &self,
        signer: &PublicKey,
        role: &GroupMemberRole,
    ) -> EyreResult<()> {
        if *role == GroupMemberRole::Admin && !self.is_admin(signer)? {
            bail!("only admins can add new admins");
        }
        Ok(())
    }

    pub fn require_admin_to_remove_admin(
        &self,
        signer: &PublicKey,
        member: &PublicKey,
    ) -> EyreResult<()> {
        if self.is_admin(member)? && !self.is_admin(signer)? {
            bail!("only admins can remove other admins");
        }
        Ok(())
    }

    pub fn require_admin_or_self(&self, signer: &PublicKey, member: &PublicKey) -> EyreResult<()> {
        if !self.is_admin(signer)? && *signer != *member {
            bail!("only group admin or the member can set member alias");
        }
        Ok(())
    }
}
