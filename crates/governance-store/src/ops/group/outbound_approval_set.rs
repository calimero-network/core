//! `GroupOp::OutboundApprovalSet` apply handler.
//!
//! Same authority as `TargetApplicationSet` (admin or `MANAGE_APPLICATION`), and
//! namespace-scoped like the TEE policies: every subgroup and context reads the
//! root's approval, so a subgroup copy would be dead data. The op replaces the
//! publisher's whole row; an empty `prefixes` revokes everything.

use super::context::GroupApplyCtx;
use crate::{NamespaceError, NamespaceRepository, OutboundApproval, OutboundApprovalRepository};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    signer_id: &str,
    prefixes: &[String],
    reviewed: &[String],
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Where the op was published decides this, not who signed it, so it is
    // refused the same way whoever the signer is.
    if NamespaceRepository::new(store).parent(group_id)?.is_some() {
        bail!(NamespaceError::OutboundApprovalNotOnSubgroup(format!(
            "{group_id:?}"
        )));
    }
    ctx.permissions()
        .require_manage_application(signer, "approve outbound requests")?;
    OutboundApprovalRepository::new(store).set(
        group_id,
        &OutboundApproval {
            signer: signer_id.to_owned(),
            prefixes: prefixes.to_vec(),
            reviewed: reviewed.to_vec(),
        },
    )
}
