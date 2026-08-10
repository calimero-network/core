//! `RootOp::MemberJoined` apply handler. Extracted from
//! `NamespaceGovernance::execute_member_joined` in #2481.

use super::context::NamespaceApplyCtx;
use crate::NamespaceMembershipService;
use calimero_account::AccountId;
use calimero_context_client::local_governance::JoinAccountCredential;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::SignedGroupOpenInvitation;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: &AccountId,
    signed_invitation: &SignedGroupOpenInvitation,
    joined_at: Option<u64>,
    account: &JoinAccountCredential,
) -> EyreResult<()> {
    let events = NamespaceMembershipService::new(ctx.store(), ctx.namespace_id())
        .apply_member_joined(&op.signer, member, signed_invitation, joined_at, account)?;
    for event in events {
        ctx.queue_event(event);
    }

    // Record the joiner's device binding in the SAME apply as its membership.
    //
    // This is what "enrolled by construction" means: there is no ordering in which
    // this member is known to the group without its account also being known, so no
    // grant can be made against a stand-in that its writes later fail to present.
    //
    // No endorsement travels on the wire, and that is not an omission.
    // `AccountDeviceLinked` needs one because an account root is a member nowhere,
    // so its gate asks whether some *member* vouched. Here the op is signed by the
    // joining member and carries the admin-signed invitation authorising them, so
    // both halves are already present; what is left is checking the certificate
    // against the genesis and that it names this joiner's own key. The endorser row
    // is still written — see `record_join_credential`.
    super::member_joined_open::record_join_credential(ctx, *member, account)?;
    Ok(())
}
