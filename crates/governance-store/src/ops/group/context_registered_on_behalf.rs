//! `GroupOp::ContextRegisteredOnBehalf` apply handler.
//!
//! The delegated sibling of `context_registered`: the same registration, with
//! the authorization decided by [`crate::creation_gate`] against the warrant's
//! author instead of against the signer.

use calimero_account::ContextCreationDelegation;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use eyre::Result as EyreResult;

use super::super::super::{now_millis, set_context_service_name};
use super::context::GroupApplyCtx;
use crate::creation_gate::{check_delegated_creation, spend_creation_nonce, ClaimedRegistration};
use crate::MetadataRepository;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    application_id: &ApplicationId,
    blob_id: &BlobId,
    service_name: &Option<String>,
    name: &Option<String>,
    delegation: &ContextCreationDelegation,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    let warrant = check_delegated_creation(
        store,
        ctx.permissions(),
        group_id,
        signer,
        ClaimedRegistration {
            context_id,
            application_id,
            service_name,
            name,
        },
        delegation,
    )?;
    // Validated before anything is written: a name the metadata rules refuse
    // must refuse the whole registration, not leave a nameless context behind.
    if name.is_some() {
        validate_metadata_payload(name.as_deref(), &Default::default())
            .map_err(|e| eyre::eyre!(e))?;
    }

    super::shared_writers_rotated::refuse_moving_rotated_context(ctx, context_id, Some(group_id))?;
    ctx.context_registration()
        .register_authorized(context_id, application_id, blob_id)?;
    if let Some(service) = service_name {
        set_context_service_name(store, context_id, service)?;
    }
    if name.is_some() {
        MetadataRepository::new(store).set_context(
            group_id,
            context_id,
            &MetadataRecord {
                name: name.clone(),
                data: Default::default(),
                updated_at: now_millis(),
                // The author's device, not the relay's key: the name is the
                // member's choice, and the warrant is what says so.
                updated_by: warrant.author_device_key,
            },
        )?;
    }
    spend_creation_nonce(store, context_id, &warrant)?;

    crate::registration_notify::notify(*context_id);
    ctx.queue_event(crate::op_events::OpEvent::ContextRegistered {
        group_id: group_id.to_bytes(),
        context_id: *context_id,
    });
    Ok(())
}
