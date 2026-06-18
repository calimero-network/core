//! `RootOp::PolicyUpdated` apply handler. Extracted from
//! `NamespaceGovernance::execute_policy_updated` in #2481.
//!
//! The policy bytes themselves are stored in the namespace DAG log;
//! no additional state mutation happens at apply time.

use super::context::NamespaceApplyCtx;
use calimero_context_client::local_governance::SignedNamespaceOp;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut NamespaceApplyCtx<'_>, op: &SignedNamespaceOp) -> EyreResult<()> {
    ctx.require_namespace_admin(&op.signer)?;
    tracing::debug!("PolicyUpdated: stored in DAG log, no additional state mutation");
    Ok(())
}
