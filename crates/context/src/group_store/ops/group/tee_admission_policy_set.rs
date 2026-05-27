//! `GroupOp::TeeAdmissionPolicySet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::group_store::{NamespaceError, NamespaceRepository};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_admin(signer)?;
    // TEE policies are namespace-scoped — refuse to apply an op
    // targeting a subgroup even if it arrives via replication.
    // Reader resolves to root anyway, so a stored subgroup op would
    // be dead data; rejecting at apply time keeps state clean.
    if NamespaceRepository::new(store).parent(group_id)?.is_some() {
        bail!(NamespaceError::TeePolicyNotOnSubgroup(format!(
            "{group_id:?}"
        )));
    }
    Ok(())
}
