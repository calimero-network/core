//! `GroupOp::MemberCapabilitySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{CapabilitiesRepository, MembershipError, MembershipRepository};
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    capabilities: &u32,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_admin(signer)?;
    // Only set capabilities on an EXISTING direct member. Otherwise a
    // capability row is written for an identity that has no member row —
    // an orphan the enumeration/authorization paths never reconcile.
    if MembershipRepository::new(store)
        .role_of(group_id, member)?
        .is_none()
    {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(group_id.to_bytes()),
            identity: format!("{member:?}"),
        });
    }
    CapabilitiesRepository::new(store).set_member_capability(group_id, member, *capabilities)?;
    Ok(())
}
