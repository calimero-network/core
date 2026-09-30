//! `GroupOp::Noop` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::account_ops::key_is_member;
use super::context::GroupApplyCtx;
use crate::MembershipError;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>) -> EyreResult<()> {
    // Changes nothing, but still grows the op log, so only a member may append one.
    if !key_is_member(ctx, ctx.signer())? {
        bail!(MembershipError::NotMember {
            group_id: hex::encode(ctx.group_id().to_bytes()),
            identity: format!("{}", ctx.signer()),
        });
    }
    Ok(())
}
