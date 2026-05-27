//! `GroupOp::ContextMetadataSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::now_millis;
use super::context::GroupApplyCtx;
use crate::group_store::{get_group_for_context, ContextRegistrationError, MetadataRepository};
use calimero_primitives::context::ContextId;
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use eyre::{bail, Result as EyreResult};
use std::collections::BTreeMap;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    name: &Option<String>,
    data: &BTreeMap<String, String>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_can_manage_metadata(signer)?;
    // Reject metadata for a context that isn't registered in this
    // group — otherwise we'd create orphaned `GroupContextMetadata`
    // rows for contexts in a different group (or no group at all).
    if get_group_for_context(store, context_id)? != Some(*group_id) {
        bail!(ContextRegistrationError::NotInGroup {
            group_id: hex::encode(group_id.to_bytes()),
            context_id: format!("{context_id:?}"),
        });
    }
    validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
    MetadataRepository::new(store).set_context(
        group_id,
        context_id,
        &MetadataRecord {
            name: name.clone(),
            data: data.clone(),
            updated_at: now_millis(),
            updated_by: *signer,
        },
    )?;
    Ok(())
}
