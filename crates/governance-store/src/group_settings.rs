use crate::{CapabilitiesRepository, MetaRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};

use super::permission_checker::PermissionChecker;

/// Group-level settings mutation service.
///
/// Encapsulates metadata/settings writes so governance mutation orchestration
/// can delegate intent-focused calls instead of inlining repeated store logic.
pub struct GroupSettingsService<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> GroupSettingsService<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn set_default_capabilities(
        &self,
        signer: &PublicKey,
        capabilities: u32,
    ) -> EyreResult<()> {
        let permissions = self.permissions();
        permissions.require_admin(signer)?;
        CapabilitiesRepository::new(self.store)
            .set_default_capabilities(&self.group_id, capabilities)
    }

    pub fn set_upgrade_policy(&self, signer: &PublicKey, policy: &UpgradePolicy) -> EyreResult<()> {
        let permissions = self.permissions();
        permissions.require_admin(signer)?;
        let mut meta = self.load_required_meta()?;
        meta.upgrade_policy = policy.clone();
        MetaRepository::new(self.store).save(&self.group_id, &meta)
    }

    pub fn set_target_application(
        &self,
        signer: &PublicKey,
        app_key: &[u8; 32],
        target_application_id: &ApplicationId,
    ) -> EyreResult<()> {
        let permissions = self.permissions();
        permissions.require_manage_application(signer, "set target application")?;
        let mut meta = self.load_required_meta()?;
        meta.app_key = *app_key;
        meta.target_application_id = *target_application_id;
        MetaRepository::new(self.store).save(&self.group_id, &meta)
    }

    pub fn set_subgroup_visibility(
        &self,
        signer: &PublicKey,
        mode: VisibilityMode,
    ) -> EyreResult<()> {
        let permissions = self.permissions();
        permissions.require_can_manage_visibility(signer)?;
        CapabilitiesRepository::new(self.store).set_subgroup_visibility(&self.group_id, mode)
    }

    pub fn set_group_migration(
        &self,
        signer: &PublicKey,
        migration: &Option<Vec<u8>>,
    ) -> EyreResult<()> {
        let permissions = self.permissions();
        permissions.require_manage_application(signer, "set group migration")?;
        let mut meta = self.load_required_meta()?;
        meta.migration = migration.clone();
        MetaRepository::new(self.store).save(&self.group_id, &meta)
    }

    fn load_required_meta(&self) -> EyreResult<GroupMetaValue> {
        MetaRepository::new(self.store)
            .load(&self.group_id)?
            .ok_or_else(|| eyre!("group metadata not found"))
    }

    fn permissions(&self) -> PermissionChecker<'a> {
        PermissionChecker::new(self.store, self.group_id)
    }
}
