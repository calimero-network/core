use crate::{CapabilitiesRepository, MetaRepository, UpgradeLadderRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupMetaValue, LadderRung};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};

use super::permission_checker::PermissionChecker;
use crate::authorizer::AtCutAuthorizer;

/// Group-level settings mutation service.
///
/// Encapsulates metadata/settings writes so governance mutation orchestration
/// can delegate intent-focused calls instead of inlining repeated store logic.
pub struct GroupSettingsService<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    /// The op's causal cut, when this service is driven from an apply path.
    /// Empty for emit-time / read-side constructions, which resolve live.
    parents: &'a [[u8; 32]],
    /// The authority decision source the settings gates consult. Defaults to the
    /// live resolver; the apply path swaps in the at-cut one via
    /// [`with_apply_auth`](Self::with_apply_auth).
    authorizer: &'a dyn AtCutAuthorizer,
}

impl<'a> GroupSettingsService<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self {
            store,
            group_id,
            parents: &[],
            authorizer: &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
        }
    }

    /// Attach the op's causal cut + at-cut authority source, so the settings gates
    /// (`require_admin`, `require_manage_application`,
    /// `require_can_manage_visibility`) decide from the projection at the op's own
    /// parents instead of the receiver's current rows.
    ///
    /// Without this, every settings op authorizes against whatever the receiver
    /// happens to have folded — so two replicas that folded different sets of
    /// concurrent capability/role ops reach opposite verdicts on the SAME op, and
    /// the one that says "unauthorized" drops it permanently. That is the same
    /// cross-replica divergence the cascade arms fixed by gating at the root, and
    /// the single-group settings ops need the identical treatment.
    #[must_use]
    pub fn with_apply_auth(
        mut self,
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn AtCutAuthorizer,
    ) -> Self {
        self.parents = parents;
        self.authorizer = authorizer;
        self
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
        // A pending migration only runs on receivers under LazyOnAccess.
        // Flipping away from it while one is pending would leave un-accessed
        // contexts swapping bytecode without migrating (silent corruption /
        // stranded state). Reject deterministically — `meta.migration` is
        // replicated, so every node folds this op to the same decision.
        if meta.migration.is_some() && !matches!(policy, UpgradePolicy::LazyOnAccess) {
            return Err(eyre!(
                "cannot set upgrade policy to {policy:?} while a migration is pending for group \
                 {:?}; only LazyOnAccess runs the pending migration on receivers — complete or \
                 abort the migration first",
                self.group_id
            ));
        }
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
        self.set_target_application_unchecked(app_key, target_application_id)
    }

    /// Write the target-application mutation WITHOUT the per-group
    /// `MANAGE_APPLICATION` check. Used by the cascade arms, which authorize
    /// the whole op ONCE against the root admin: re-deriving live per-descendant
    /// authority here would make the apply/bail outcome depend on each replica's
    /// fold progress and diverge (see the cascade apply arms).
    ///
    /// Caller contract: invoke ONLY after a single deterministic root-authority
    /// check (`ctx.permissions().require_manage_application`). Never from a
    /// single-group op arm — use the checked [`Self::set_target_application`].
    pub(crate) fn set_target_application_unchecked(
        &self,
        app_key: &[u8; 32],
        target_application_id: &ApplicationId,
    ) -> EyreResult<()> {
        let mut meta = self.load_required_meta()?;
        meta.app_key = *app_key;
        meta.target_application_id = *target_application_id;
        // Append the ladder rung BEFORE advancing meta. This is the one capture
        // point for the upgrade ladder a behind context replays rung by rung,
        // and the ordering matters when the two writes can't be atomic: a
        // rung-without-advanced-meta (append ok, save fails) is recoverable
        // (the target still points at the old version, a retry re-appends),
        // whereas advanced-meta-without-a-rung would strand a behind context
        // with no hop to replay (NoMigrationPath).
        UpgradeLadderRepository::new(self.store).append(
            &self.group_id,
            LadderRung {
                app_key: *app_key,
                application_id: *target_application_id,
            },
        )?;
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
        self.set_group_migration_unchecked(migration)
    }

    /// Migration analogue of [`set_target_application_unchecked`]: writes the
    /// mutation without the per-group `MANAGE_APPLICATION` check, for the
    /// cascade arms that authorize once against the root admin.
    pub(crate) fn set_group_migration_unchecked(
        &self,
        migration: &Option<Vec<u8>>,
    ) -> EyreResult<()> {
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
            .with_apply_auth(self.parents, self.authorizer)
    }
}
