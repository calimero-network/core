//! `GroupOp::TargetApplicationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MetaRepository, MetadataRepository};
use calimero_primitives::application::ApplicationId;
use calimero_store::key::ApplicationMeta;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // The group's pre-upgrade target, for the announcement's `from` side. Read
    // before the mutation below overwrites it. Announcing is observational, so
    // every read it needs stays non-fatal: a failed one drops the event, it
    // never fails the apply and diverges this replica.
    let previous_application_id = MetaRepository::new(store)
        .load(group_id)
        .ok()
        .flatten()
        .map(|meta| meta.target_application_id);

    ctx.settings()
        .set_target_application(signer, app_key, target_application_id)?;

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
    if target_blob.is_some_and(|blob| blob != *app_key) {
        return Ok(());
    }
    let local_contexts_total = MetadataRepository::new(store)
        .count_contexts(group_id)
        .unwrap_or_default() as u32;
    ctx.queue_migration_started(
        previous_application_id.as_ref(),
        target_application_id,
        None,
        local_contexts_total,
    );
    Ok(())
}
