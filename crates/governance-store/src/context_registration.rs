use crate::{MetaRepository, MetadataRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{ApplicationMeta, ContextMeta};
use calimero_store::types;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::permission_checker::PermissionChecker;
use super::{context_tree::ContextTreeService, get_group_for_context};

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
        blob_id: &BlobId,
    ) -> EyreResult<()> {
        permissions.require_can_create_context(signer)?;
        tracing::info!(
            %context_id,
            %application_id,
            group_id = %hex::encode(self.group_id.to_bytes()),
            "processing ContextRegistered governance op"
        );

        ContextTreeService::new(self.store, self.group_id).register_context(context_id)?;
        self.backfill_application_if_needed(context_id, application_id, blob_id)
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
                ContextTreeService::new(self.store, self.group_id)
                    .unregister_context(context_id)?;
                // Drop the context's metadata record so detach doesn't leave
                // an orphaned `GroupContextMetadata` row behind.
                MetadataRepository::new(self.store).delete_context(&self.group_id, context_id)?;
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
        blob_id: &BlobId,
    ) -> EyreResult<()> {
        if *application_id == ZERO_APPLICATION_ID {
            return Ok(());
        }

        if let Some(meta) = MetaRepository::new(self.store).load(&self.group_id)? {
            // `bytecode_id` is healed alongside the application because it is the
            // same quantity an invitation carries: both are the creator's
            // `app_meta.bytecode` blob. A node that gained the namespace by key
            // delivery rather than by invitation - a paired device - otherwise
            // keeps a zero here forever, which disarms every path keyed on the
            // group's target blob. It is not folded into `hash_group_state`, so
            // healing it locally cannot diverge this replica.
            let heal_application = meta.target.application_id == ZERO_APPLICATION_ID;
            let heal_bytecode = meta.target.bytecode_id == [0u8; 32];
            if heal_application || heal_bytecode {
                let mut updated = meta;
                if heal_application {
                    updated.target.application_id = *application_id;
                }
                if heal_bytecode {
                    updated.target.bytecode_id = **blob_id;
                }
                MetaRepository::new(self.store).save(&self.group_id, &updated)?;
                tracing::info!(
                    group_id = %hex::encode(self.group_id.to_bytes()),
                    %application_id,
                    %blob_id,
                    heal_application,
                    heal_bytecode,
                    "healed group meta from ContextRegistered"
                );
            }
        }

        let ctx_meta_key = ContextMeta::new(*context_id);
        let mut handle = self.store.handle();
        if let Ok(Some(mut ctx_meta)) = handle.get(&ctx_meta_key) {
            let ctx_meta: &mut types::ContextMeta = &mut ctx_meta;
            if ctx_meta.application.application_id() == ZERO_APPLICATION_ID {
                *ctx_meta = types::ContextMeta::new(
                    ApplicationMeta::new(*application_id),
                    ctx_meta.root_hash,
                    ctx_meta.dag_heads.clone(),
                    ctx_meta.service_name.clone(),
                );
                handle.put(&ctx_meta_key, ctx_meta)?;
            }
        }

        Ok(())
    }
}
