//! `GroupOp::ContextRegistered` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::set_context_service_name;
use super::context::GroupApplyCtx;
use crate::context_registration::registered_elsewhere_at_cut;
use crate::errors::ContextRegistrationError;
use crate::NamespaceRepository;
use calimero_app_downloader::registry::RegistryCoords;
use calimero_governance_types::NamespaceId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    application_id: &ApplicationId,
    blob_id: &BlobId,
    service_name: &Option<String>,
    coords: RegistryCoords<'_>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();
    refuse_registered_elsewhere(ctx, context_id)?;
    super::shared_writers_rotated::refuse_moving_rotated_context(ctx, context_id, Some(group_id))?;

    let permissions = ctx.permissions();
    ctx.context_registration().register(
        permissions,
        signer,
        context_id,
        application_id,
        blob_id,
        coords,
    )?;
    if let Some(name) = service_name {
        set_context_service_name(store, context_id, name)?;
    }
    // Signal any waiters (e.g. `join_context` racing against gossipsub
    // propagation) that the context→group mapping has just been
    // persisted. See `crate::registration_notify` for rationale.
    crate::registration_notify::notify(*context_id);
    ctx.queue_event(crate::op_events::OpEvent::ContextRegistered {
        group_id: group_id.to_bytes(),
        context_id: *context_id,
    });
    Ok(())
}

/// Refuse a context the op's cut already gave to another group: the first group owns it.
pub(super) fn refuse_registered_elsewhere(
    ctx: &GroupApplyCtx<'_>,
    context_id: &ContextId,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace = NamespaceRepository::new(store).resolve(ctx.group_id())?;
    if registered_elsewhere_at_cut(
        store,
        NamespaceId::from(namespace.to_bytes()),
        ctx.group_id(),
        context_id,
        ctx.cut(),
        ctx.authorizer(),
    )? {
        bail!(ContextRegistrationError::RegisteredElsewhere {
            context_id: context_id.to_string(),
        });
    }
    Ok(())
}
