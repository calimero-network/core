//! `GroupOp::ContextRegistered` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::set_context_service_name;
use super::context::GroupApplyCtx;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    application_id: &ApplicationId,
    service_name: &Option<String>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    let permissions = ctx.permissions();
    ctx.context_registration()
        .register(permissions, signer, context_id, application_id)?;
    if let Some(name) = service_name {
        set_context_service_name(store, context_id, name)?;
    }
    // Signal any waiters (e.g. `join_context` racing against gossipsub
    // propagation) that the context→group mapping has just been
    // persisted. See `crate::registration_notify` for rationale.
    crate::registration_notify::notify(*context_id);
    crate::op_events::notify(crate::op_events::OpEvent::ContextRegistered {
        group_id: group_id.to_bytes(),
        context_id: *context_id,
    });
    Ok(())
}
