//! `RootOp::MemberJoined` apply handler. Extracted from
//! `NamespaceGovernance::execute_member_joined` in #2481.

use super::context::NamespaceApplyCtx;
use crate::{BindingRejected, NamespaceMembershipService};
use calimero_account::AccountId;
use calimero_context_client::local_governance::JoinAccountCredential;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::{ContextGroupId, SignedGroupOpenInvitation};
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: &AccountId,
    signed_invitation: &SignedGroupOpenInvitation,
    joined_at: Option<u64>,
    account: &JoinAccountCredential,
    admitter_endorsement: Option<&calimero_governance_types::AdmitterEndorsement>,
) -> EyreResult<()> {
    // The inviter's authority is decided at this op's cut, not this replica's
    // current rows: see `NamespaceMembershipService::with_apply_auth`.
    let (parents, authorizer) = ctx.apply_auth();
    let events = NamespaceMembershipService::new(ctx.store(), ctx.namespace_id())
        .with_apply_auth(parents, authorizer)
        .apply_member_joined(
            &op.signer,
            member,
            signed_invitation,
            joined_at,
            account,
            admitter_endorsement,
        )?;
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
    record_join_credential(ctx, *member, account)?;
    Ok(())
}

/// Record a joiner's certified account alongside its membership.
///
/// Shared by [`apply`] and the TEE admission. Reports a
/// credential refusal instead of propagating it: a credential this group cannot
/// admit must not orphan the membership op behind it. A member with no binding is
/// the state that held before joins carried one at all — survivable; a member the
/// DAG cannot apply at all is not.
///
/// A **store** failure is the opposite case and does propagate. It is not a verdict
/// on the credential, and swallowing it would leave this replica holding the
/// membership with no binding while its peers hold both — a permanent, silent
/// disagreement about which principal that member writes as. Returning the error
/// leaves the head unadvanced so the op is retried, which is what a transient
/// failure deserves and what a persistent one should be loud about.
///
/// # Errors
/// Propagates a store read/write failure from the binding or endorser write.
pub(super) fn record_join_credential(
    ctx: &mut NamespaceApplyCtx<'_>,
    member: AccountId,
    account: &JoinAccountCredential,
) -> EyreResult<()> {
    let namespace = ContextGroupId::from(ctx.namespace_id().to_bytes());

    // The credential has to be the JOINER'S OWN and internally valid, and this is
    // the check that establishes both. `apply_link` below verifies the certificate
    // too, but says nothing about WHOSE it is: these ops are cleartext, so any
    // node can lift a credential out of somebody else's join and present it as the
    // `account` of its own, signed honestly with its own key. A valid outer
    // signature proves the envelope, never the payload. Binding it anyway would
    // graft a stranger's account into this namespace under the replayer's
    // membership, and — because the endorser row below is what makes a member
    // account-addressed — would aim the replayer's scope keys at the victim's
    // devices.
    //
    // Shared verbatim with the projection encoder, which has to reach the same
    // verdict or the two planes disagree about the same op in the opposite
    // direction: a credential refused a binding here and granted one in the fold.
    if !calimero_op_adapter::join_credential_binds(&member, account) {
        tracing::warn!(
            ?namespace,
            ?member,
            cert_sign_pk = %account.statement.sign_pk,
            "member joined presenting a credential that is not its own or does not \
             verify; the credential is ignored and the member is recorded without a \
             binding"
        );
        return Ok(());
    }

    let bindings = crate::AccountBindingRepository::new(ctx.store());
    let outcome = bindings.apply_link(
        &namespace,
        &account.genesis,
        &account.chain,
        &account.statement,
        crate::JOIN_SCOPE_EPOCH,
    )?;

    // The endorsement, materialized. `AccountDeviceLinked` carries an explicit
    // `AccountMemberEndorsement` because an account root is a member nowhere, so
    // its gate has to ask whether some member vouched. A join already answers
    // that: it is signed by the joining member, it carries the admin-signed
    // invitation authorising them, and the check above proves the certificate is
    // theirs. So no endorsement travels on the wire, but the ROW is written all
    // the same, so the endorser set is the same however a device was linked.
    if !outcome
        .as_ref()
        .err()
        .is_some_and(BindingRejected::is_permanent)
    {
        bindings.record_endorser(&namespace, account.statement.account, &member)?;
    }

    if let Err(rejected) = outcome {
        tracing::warn!(
            ?namespace,
            %member,
            ?rejected,
            "member joined but its account credential was refused; the member is \
             recorded without a binding, so its writes will attribute to a stand-in"
        );
    }
    Ok(())
}
