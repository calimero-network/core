use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::membership::is_group_admin_or_has_capability;
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
        self.membership.is_admin(identity)
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        self.membership.require_admin(identity)
    }

    pub fn require_manage_members(&self, identity: &PublicKey, operation: &str) -> EyreResult<()> {
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
        require_group_admin_or_capability(
            self.store,
            &self.group_id,
            identity,
            MemberCapabilities::MANAGE_APPLICATION,
            operation,
        )
    }

    pub fn require_can_create_context(&self, identity: &PublicKey) -> EyreResult<()> {
        if is_group_admin_or_has_capability(
            self.store,
            &self.group_id,
            identity,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )? {
            return Ok(());
        }

        bail!("only group admin or members with CAN_CREATE_CONTEXT can register a context")
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
