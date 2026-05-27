//! `GroupOp::GroupMetadataSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::now_millis;
use super::context::GroupApplyCtx;
use crate::group_store::MetadataRepository;
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use eyre::Result as EyreResult;
use std::collections::BTreeMap;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    name: &Option<String>,
    data: &BTreeMap<String, String>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_can_manage_metadata(signer)?;
    validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
    MetadataRepository::new(store).set_group(
        group_id,
        &MetadataRecord {
            name: name.clone(),
            data: data.clone(),
            updated_at: now_millis(),
            updated_by: *signer,
        },
    )?;
    Ok(())
}
