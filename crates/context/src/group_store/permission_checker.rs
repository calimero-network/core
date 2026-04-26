use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::membership::{
    check_group_membership_path, is_group_admin_or_has_capability, MembershipPath,
};
use super::{membership_view::GroupMembershipView, require_group_admin_or_capability};

/// Authorization service for group governance operations.
///
/// This object centralizes permission checks so callers can express intent
/// (`require_manage_members`, `require_can_create_context`) instead of wiring
/// capability bits and error messages at each callsite.
pub struct PermissionChecker<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    membership: GroupMembershipView<'a>,
}

impl<'a> PermissionChecker<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self {
            store,
            group_id,
            membership: GroupMembershipView::new(store, group_id),
        }
    }

    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        if self.membership.is_admin(identity)? {
            return Ok(true);
        }
        // Issue #2256: admin authority cascades into Open subgroups from
        // any ancestor where the signer is a direct admin (matches the
        // membership-inheritance walk used by `check_group_membership`).
        if let MembershipPath::Inherited {
            via_admin: true, ..
        } = check_group_membership_path(self.store, &self.group_id, identity)?
        {
            return Ok(true);
        }
        Ok(false)
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_admin(identity)? {
            return Ok(());
        }
        // Re-emit the direct error for non-Open / non-inherited cases so
        // existing callers see the same diagnostic surface they expect.
        self.membership.require_admin(identity)
    }

    pub fn require_manage_members(&self, identity: &PublicKey, operation: &str) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_MEMBERS)? {
            return Ok(());
        }
        require_group_admin_or_capability(
            self.store,
            &self.group_id,
            identity,
            MemberCapabilities::MANAGE_MEMBERS,
            operation,
        )
    }

    pub fn require_manage_application(
        &self,
        identity: &PublicKey,
        operation: &str,
    ) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_APPLICATION)? {
            return Ok(());
        }
        require_group_admin_or_capability(
            self.store,
            &self.group_id,
            identity,
            MemberCapabilities::MANAGE_APPLICATION,
            operation,
        )
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
    /// Direct authority in `self.group_id` short-circuits. Otherwise, if
    /// the subgroup is reachable via the parent-walk inheritance path
    /// (i.e. the subgroup or some ancestor is `Open` and `identity`
    /// anchors at a parent), the check is *re-evaluated at the anchor
    /// parent*. Admins at the anchor inherit unconditionally; non-admins
    /// must hold both `CAN_JOIN_OPEN_SUBGROUPS` (already verified by the
    /// walk) and the specific `capability_bit` at the anchor.
    fn is_authorized_with_capability(
        &self,
        identity: &PublicKey,
        capability_bit: u32,
    ) -> EyreResult<bool> {
        if is_group_admin_or_has_capability(self.store, &self.group_id, identity, capability_bit)? {
            return Ok(true);
        }
        match check_group_membership_path(self.store, &self.group_id, identity)? {
            MembershipPath::Inherited {
                via_admin: true, ..
            } => Ok(true),
            MembershipPath::Inherited {
                anchor,
                via_admin: false,
            } => is_group_admin_or_has_capability(self.store, &anchor, identity, capability_bit),
            _ => Ok(false),
        }
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
