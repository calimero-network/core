//! `GroupOp::TargetApplicationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::{seed_target_application_row, GroupApplyCtx};
use crate::{MetaRepository, MetadataRepository};
use calimero_app_downloader::registry::RegistryCoords;
use calimero_primitives::application::{ApplicationId, ZERO_APPLICATION_ID};
use calimero_store::key::ApplicationMeta;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    bytecode_id: &[u8; 32],
    target_application_id: &ApplicationId,
    coords: RegistryCoords<'_>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Read before the mutation overwrites it. Announcing is observational, so a
    // failed read drops the event rather than failing the apply.
    let previous_target = MetaRepository::new(store)
        .load(group_id)
        .ok()
        .flatten()
        .map(|meta| meta.target);

    ctx.settings()
        .set_target_application(signer, bytecode_id, target_application_id, coords)?;

    seed_target_application_row(store, target_application_id, bytecode_id, coords)?;

    // A group's first target, or a restated one, is not a migration.
    let moved = previous_target.as_ref().is_none_or(|previous| {
        previous.application_id != ZERO_APPLICATION_ID
            && (previous.application_id != *target_application_id
                || previous.bytecode_id != *bytecode_id)
    });
    if !moved {
        return Ok(());
    }

    // A multi-hop upgrade emits one of these ops per ladder rung, all naming
    // the same target application. Announce on the rung that actually lands the
    // group on the target's bytecode, so members see one migration, not one per
    // hop. An unknown target row cannot be compared - announce rather than go
    // silent.
    let target_blob = store
        .handle()
        .get(&ApplicationMeta::new(*target_application_id))
        .ok()
        .flatten()
        .map(|app| *app.bytecode.blob_id().as_ref());
    if target_blob.is_some_and(|blob| blob != *bytecode_id) {
        return Ok(());
    }
    let local_contexts_total = MetadataRepository::new(store)
        .count_contexts(group_id)
        .unwrap_or_default() as u32;
    ctx.queue_migration_started(
        previous_target
            .as_ref()
            .map(|target| &target.application_id),
        target_application_id,
        None,
        local_contexts_total,
    );
    Ok(())
}
