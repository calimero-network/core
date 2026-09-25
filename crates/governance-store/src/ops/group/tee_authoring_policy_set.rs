//! `GroupOp::TeeAuthoringPolicySet` apply handler.
//!
//! Same authorization shape as `TeeAdmissionPolicySet`: an admin signs it and it
//! lives on the namespace root. The policy itself is not materialized — the op
//! log is the storage, read back by `read_tee_authoring_policy`.

use super::context::GroupApplyCtx;
use crate::{NamespaceError, NamespaceRepository};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    ctx.permissions().require_admin(signer)?;
    // The reader resolves to the root, so a subgroup copy would be dead data
    // that reads as if it meant something. Refuse it at apply instead.
    if NamespaceRepository::new(store).parent(group_id)?.is_some() {
        bail!(NamespaceError::TeeAuthoringPolicyNotOnSubgroup(format!(
            "{group_id:?}"
        )));
    }
    Ok(())
}
