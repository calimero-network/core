use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{check_group_membership, get_group_member_role, is_group_admin, list_group_members};

/// Read-model for group membership lookups and derived checks.
pub struct GroupMembershipView<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> GroupMembershipView<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn is_admin(&self, member: &PublicKey) -> EyreResult<bool> {
        is_group_admin(self.store, &self.group_id, member)
    }

    pub fn role_of(&self, member: &PublicKey) -> EyreResult<Option<GroupMemberRole>> {
        get_group_member_role(self.store, &self.group_id, member)
    }

    pub fn is_member(&self, member: &PublicKey) -> EyreResult<bool> {
        check_group_membership(self.store, &self.group_id, member)
    }

    pub fn admin_count(&self) -> EyreResult<usize> {
        self.list_members()?
            .iter()
            .try_fold(0usize, |count, row| match row.1 {
                GroupMemberRole::Admin => Ok(count + 1),
                _ => Ok(count),
            })
    }

    pub fn has_another_admin(&self, excluded: &PublicKey) -> EyreResult<bool> {
        Ok(self
            .list_members()?
            .into_iter()
            .any(|(member, role)| role == GroupMemberRole::Admin && member != *excluded))
    }

    pub fn member_count(&self) -> EyreResult<usize> {
        Ok(self.list_members()?.len())
    }

    pub fn list_members(&self) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        list_group_members(self.store, &self.group_id, 0, usize::MAX)
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_admin(identity)? {
            return Ok(());
        }
        bail!("identity {identity} is not an admin of this group")
    }
}
