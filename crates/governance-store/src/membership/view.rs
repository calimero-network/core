use crate::{MembershipError, MembershipRepository, MetaRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

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
        MembershipRepository::new(self.store).is_admin(&self.group_id, member)
    }

    pub fn role_of(&self, member: &PublicKey) -> EyreResult<Option<GroupMemberRole>> {
        MembershipRepository::new(self.store).role_of(&self.group_id, member)
    }

    pub fn is_member(&self, member: &PublicKey) -> EyreResult<bool> {
        MembershipRepository::new(self.store).is_member(&self.group_id, member)
    }

    /// Answers "is there an admin other than `excluded`". Self-enforces that
    /// `excluded` is itself an admin: a non-admin can never be the last admin,
    /// so we return `false` for it rather than treating the genesis founder as
    /// the "other" admin.
    pub fn has_another_admin(&self, excluded: &PublicKey) -> EyreResult<bool> {
        if !self.is_admin(excluded)? {
            return Ok(false);
        }
        if self
            .list_members()?
            .into_iter()
            .any(|(member, role)| role == GroupMemberRole::Admin && member != *excluded)
        {
            return Ok(true);
        }
        // also count the genesis founder (meta.admin_identity), which has no stored row
        Ok(MetaRepository::new(self.store)
            .load(&self.group_id)?
            .is_some_and(|meta| meta.admin_identity != *excluded))
    }

    pub fn list_members(&self) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        MembershipRepository::new(self.store).list(&self.group_id, 0, usize::MAX)
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_admin(identity)? {
            return Ok(());
        }
        bail!(MembershipError::NotAdmin {
            group_id: format!("{:?}", self.group_id),
            identity: format!("{identity:?}"),
        })
    }
}
