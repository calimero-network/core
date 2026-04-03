use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{ApplicationMeta, ContextMeta};
use calimero_store::types;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::permission_checker::PermissionChecker;
use super::{
    get_group_for_context, load_group_meta, register_context_in_group, save_group_meta,
    unregister_context_from_group,
};

/// Service that applies context registration and detachment mutations.
pub struct ContextRegistrationService<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> ContextRegistrationService<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn register(
        &self,
        permissions: &PermissionChecker<'_>,
        signer: &PublicKey,
        context_id: &ContextId,
        application_id: &ApplicationId,
    ) -> EyreResult<()> {
        permissions.require_can_create_context(signer)?;
        tracing::info!(
            %context_id,
            %application_id,
            group_id = %hex::encode(self.group_id.to_bytes()),
            "processing ContextRegistered governance op"
        );

        register_context_in_group(self.store, &self.group_id, context_id)?;
        self.backfill_application_if_needed(context_id, application_id)
    }

    pub fn detach(
        &self,
        permissions: &PermissionChecker<'_>,
        signer: &PublicKey,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        permissions.require_admin(signer)?;
        match get_group_for_context(self.store, context_id)? {
            Some(g) if g == self.group_id => {
                unregister_context_from_group(self.store, &self.group_id, context_id)?;
                Ok(())
            }
            Some(_) => bail!("context is registered to a different group"),
            None => bail!("context is not registered in any group"),
        }
    }

    fn backfill_application_if_needed(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
    ) -> EyreResult<()> {
        if *application_id == ZERO_APPLICATION_ID {
            return Ok(());
        }

        if let Some(meta) = load_group_meta(self.store, &self.group_id)? {
            if meta.target_application_id == ZERO_APPLICATION_ID {
                let mut updated = meta;
                updated.target_application_id = *application_id;
                save_group_meta(self.store, &self.group_id, &updated)?;
                tracing::info!(
                    group_id = %hex::encode(self.group_id.to_bytes()),
                    %application_id,
                    "updated group meta with real application ID from ContextRegistered"
                );
            }
        }

        let ctx_meta_key = ContextMeta::new(*context_id);
        let handle = self.store.handle();
        if let Ok(Some(mut ctx_meta)) = handle.get(&ctx_meta_key) {
            let ctx_meta: &mut types::ContextMeta = &mut ctx_meta;
            if ctx_meta.application.application_id() == ZERO_APPLICATION_ID {
                *ctx_meta = types::ContextMeta::new(
                    ApplicationMeta::new(*application_id),
                    ctx_meta.root_hash,
                    ctx_meta.dag_heads.clone(),
                    ctx_meta.service_name.clone(),
                );
                drop(handle);
                let mut wh = self.store.handle();
                wh.put(&ctx_meta_key, ctx_meta)?;
            }
        }

        Ok(())
    }
}
