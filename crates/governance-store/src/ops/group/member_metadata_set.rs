//! `GroupOp::MemberMetadataSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::now_millis;
use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository, MetadataRepository};
use calimero_primitives::identity::PublicKey;
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use eyre::{bail, Result as EyreResult};
use std::collections::BTreeMap;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    name: &Option<String>,
    data: &BTreeMap<String, String>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // A member may always set *their own* metadata — but only if they
    // actually are a member of this group; otherwise this is gated like
    // the other metadata ops (admin or CAN_MANAGE_METADATA).
    if signer == member {
        if !MembershipRepository::new(store).is_member(group_id, signer)? {
            bail!(MembershipError::NotMember {
                group_id: hex::encode(group_id.to_bytes()),
                identity: format!("{signer:?}"),
            });
        }
    } else {
        ctx.permissions().require_can_manage_metadata(signer)?;
    }
    validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
    MetadataRepository::new(store).set_member(
        group_id,
        member,
        &MetadataRecord {
            name: name.clone(),
            data: data.clone(),
            updated_at: now_millis(),
            updated_by: *signer,
        },
    )?;
    Ok(())
}
