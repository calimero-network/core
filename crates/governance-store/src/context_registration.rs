use std::collections::{HashMap, HashSet};

use crate::authorizer::AtCutAuthorizer;
use crate::namespace::{order_causally, NamespaceOpLogService};
use crate::void_ledger::VoidLedger;
use crate::{MetaRepository, MetadataRepository, NamespaceRepository, OpenedNamespaceOp};
use calimero_app_downloader::registry::{stored_coords, RegistryCoords};
use calimero_context_client::local_governance::{GroupOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
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
        coords: RegistryCoords<'_>,
    ) -> EyreResult<()> {
        permissions.require_can_create_context(signer)?;
        self.register_authorized(context_id, application_id, blob_id, coords)
    }

    /// Register a context whose authorization has already been decided.
    ///
    /// Split from [`Self::register`] for the delegated registration, whose
    /// authority is the warrant author's rather than the signer's and is
    /// checked by [`crate::creation_gate`] before this runs. Crate-private so
    /// no caller can reach the write without one of the two gates.
    pub(crate) fn register_authorized(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        blob_id: &BlobId,
        coords: RegistryCoords<'_>,
    ) -> EyreResult<()> {
        tracing::info!(
            %context_id,
            %application_id,
            group_id = %hex::encode(self.group_id.to_bytes()),
            "processing ContextRegistered governance op"
        );

        ContextTreeService::new(self.store, self.group_id).register_context(context_id)?;
        self.backfill_application_if_needed(context_id, application_id, blob_id, coords)
    }

    /// Detach `context_id`, which `registered` says the op's cut holds in this group.
    pub fn detach(
        &self,
        permissions: &PermissionChecker<'_>,
        signer: &PublicKey,
        context_id: &ContextId,
        registered: bool,
    ) -> EyreResult<()> {
        permissions.require_admin(signer)?;
        if !registered {
            bail!("context is not registered in this group");
        }
        // A concurrent detach may have taken it already.
        if get_group_for_context(self.store, context_id)? == Some(self.group_id) {
            ContextTreeService::new(self.store, self.group_id).unregister_context(context_id)?;
            // Drop the context's metadata record so detach doesn't leave
            // an orphaned `GroupContextMetadata` row behind.
            MetadataRepository::new(self.store).delete_context(&self.group_id, context_id)?;
        }
        Ok(())
    }

    fn backfill_application_if_needed(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        blob_id: &BlobId,
        coords: RegistryCoords<'_>,
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
            // The coordinates are what the lazy upgrade fetches the target by, so
            // they only join a blob this op names.
            let heal_coords = (heal_bytecode || meta.target.bytecode_id == **blob_id)
                && stored_coords(&meta.target.package, &meta.target.version).is_none()
                && stored_coords(coords.package, coords.version).is_some();
            if heal_application || heal_bytecode || heal_coords {
                let mut updated = meta;
                if heal_application {
                    updated.target.application_id = *application_id;
                }
                if heal_bytecode {
                    updated.target.bytecode_id = **blob_id;
                }
                if heal_coords {
                    updated.target.package = coords.package.into();
                    updated.target.version = coords.version.into();
                }
                MetaRepository::new(self.store).save(&self.group_id, &updated)?;
                tracing::info!(
                    group_id = %hex::encode(self.group_id.to_bytes()),
                    %application_id,
                    %blob_id,
                    heal_application,
                    heal_bytecode,
                    heal_coords,
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

/// Whether `context` is registered in `group` at the cut `parents`, or live where that is
/// sound. A detach concurrent with the op must not refuse it on the node that took it first.
pub(crate) fn registered_at_cut(
    store: &Store,
    group: &ContextGroupId,
    context: &ContextId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<bool> {
    if !parents.is_empty() {
        if let Some(registered) = registration_in_log(store, group, context, parents)? {
            return Ok(registered);
        }
        if !authorizer.can_resolve_cut(group, parents) {
            bail!(crate::ApplyError::AuthorityUndecidable {
                group_id: group.to_string(),
                signer: "the context's registration at the op's cut".to_owned(),
            });
        }
    }
    Ok(get_group_for_context(store, context)? == Some(*group))
}

/// [`registered_at_cut`] from `group`'s applied registrations and detaches of `context`: the
/// causally latest in the cut, else the first outside it tells what the rows began as.
/// `None` when the log moves nothing, or an op of `group` or the cut does not read here.
fn registration_in_log(
    store: &Store,
    group: &ContextGroupId,
    context: &ContextId,
    parents: &[[u8; 32]],
) -> EyreResult<Option<bool>> {
    let namespace = NamespaceId::from(NamespaceRepository::new(store).resolve(group)?.to_bytes());
    let op_log = NamespaceOpLogService::new(store, namespace);
    let voided = VoidLedger::new(store, namespace).voided()?;
    let mut registers = HashMap::new();
    let mut changes = Vec::new();
    for entry in op_log.collect_signed_group_ops_for_group(group.to_bytes())? {
        let id = entry
            .signed_op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let opened = OpenedNamespaceOp::open(store, &entry.signed_op);
        if opened.refused || voided.contains(&id) {
            continue;
        }
        let Some(op) = opened.group_op else {
            return Ok(None);
        };
        let op = match op.into_unguarded() {
            GroupOp::OnBehalf { op, .. } => *op,
            op => op,
        };
        let moved = match op {
            GroupOp::ContextRegistered { context_id, .. }
            | GroupOp::ContextRegisteredOnBehalf { context_id, .. } => {
                (context_id == *context, true)
            }
            GroupOp::ContextDetached { context_id } => (context_id == *context, false),
            _ => continue,
        };
        if moved.0 {
            let _ = registers.insert(id, moved.1);
            changes.push(entry.signed_op);
        }
    }
    if changes.is_empty() {
        return Ok(None);
    }
    let Some(ancestry) = ancestry_of(&op_log, parents)? else {
        return Ok(None);
    };
    let (inside, outside): (Vec<_>, Vec<_>) = changes
        .into_iter()
        .partition(|op| op.content_hash().is_ok_and(|id| ancestry.contains(&id)));
    let registered_by = |op: &SignedNamespaceOp| -> EyreResult<bool> {
        let id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        Ok(registers[&id])
    };
    if let Some(latest) = order_causally(&op_log, inside)?.pop() {
        return registered_by(&latest).map(Some);
    }
    // A detach is the first move only of a context the rows began with.
    match order_causally(&op_log, outside)?.first() {
        Some(first) => registered_by(first).map(|registers| Some(!registers)),
        None => Ok(None),
    }
}

/// Every op in the ancestry of `parents`; `None` when one is missing here.
pub(crate) fn ancestry_of(
    op_log: &NamespaceOpLogService<'_>,
    parents: &[[u8; 32]],
) -> EyreResult<Option<HashSet<[u8; 32]>>> {
    let mut seen = HashSet::new();
    let mut queue = parents.to_vec();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(op) = op_log.get_signed_op(id)? else {
            return Ok(None);
        };
        queue.extend(op.parent_op_hashes);
    }
    Ok(Some(seen))
}
