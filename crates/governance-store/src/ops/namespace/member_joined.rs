//! `RootOp::MemberJoined` apply handler. Extracted from
//! `NamespaceGovernance::execute_member_joined` in #2481.

use super::context::NamespaceApplyCtx;
use crate::NamespaceMembershipService;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: &PublicKey,
    signed_invitation: &SignedGroupOpenInvitation,
    joined_at: Option<u64>,
) -> EyreResult<()> {
    let event = NamespaceMembershipService::new(ctx.store(), ctx.namespace_id())
        .apply_member_joined(&op.signer, member, signed_invitation, joined_at)?;
    if let Some(event) = event {
        ctx.queue_event(event);
    }
    Ok(())
}
