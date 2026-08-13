//! `RootOp::MemberJoinedViaTeeAttestation` apply handler — the cleartext TEE
//! fleet admission.
//!
//! The same admission decision as the encrypted
//! [`GroupOp::MemberJoinedViaTeeAttestation`](calimero_context_client::local_governance::GroupOp::MemberJoinedViaTeeAttestation)
//! handler, with one addition that is the whole reason this form exists: the op
//! is readable by every peer, so it can carry the joiner's account credential
//! and bind its device in the same apply as the membership.
//!
//! **The signer is the verifier, not the joiner.** A replica cannot admit
//! itself — some existing member has to check its quote against the namespace's
//! policy first — so `op.signer` is that member and `member` is the attested
//! key. Every other join op in this crate asserts `op.signer == member`; this
//! one must not, and the verifier-membership gate below is what stands in its
//! place.

use super::context::NamespaceApplyCtx;
use crate::membership::{MembershipPolicy, TeeAttestationClaims};
use crate::{DenyListRepository, MembershipError, ReentryRepository};
use calimero_context_client::local_governance::{JoinAccountCredential, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupExitReason;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    group_id: ContextGroupId,
    member: &PublicKey,
    claims: &TeeAttestationClaims<'_>,
    role: &GroupMemberRole,
    account: &JoinAccountCredential,
) -> EyreResult<()> {
    let signer = op.signer;
    let store = ctx.store();

    // Cross-namespace forgery guard, as on the sibling `MemberJoinedOpen`: a
    // `RootOp` is applied in the namespace it was published to, so a `group_id`
    // belonging to a DIFFERENT namespace must not be admitted into this one.
    // The encrypted form got this for free from the envelope it travelled in.
    let resolved_ns = crate::NamespaceRepository::new(store).resolve(&group_id)?;
    if resolved_ns.to_bytes() != ctx.namespace_id().to_bytes() {
        bail!(MembershipError::TeeAdmissionWrongNamespace {
            group_id: format!("{group_id:?}"),
            resolved_ns: format!("{resolved_ns:?}"),
        });
    }

    if *role != GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::TeeRoleMustBeReadOnly);
    }

    // The membership this admits is recorded against an ACCOUNT, and the only
    // thing here that names one is the credential. So the credential is now
    // load-bearing rather than merely carried: it must certify the very key the
    // quote attested, or the verifier could pair a genuine quote for one replica
    // with a credential minted for a different account and admit the wrong
    // principal on the strength of somebody else's attestation.
    //
    // This is the `cert.sign_pk == member` half of the join predicate. The other
    // half — that the OP was signed by that device — is deliberately absent
    // here and cannot be asked: a replica does not admit itself, so `op.signer`
    // is the verifying member. `require_tee_attestation_verifier_membership`
    // below is what stands in its place.
    if !calimero_op_adapter::join_credential_certifies(
        member,
        &account.genesis,
        &account.chain,
        &account.cert,
    ) {
        bail!(MembershipError::TeeCredentialNotTheAttestedKey {
            member: format!("{member}"),
        });
    }
    let member_account = account.cert.account;

    let policy_gate = MembershipPolicy::new(store, group_id);
    // The verifier vouched for the quote, and vouching is a membership act, so
    // its key resolves to the account membership is recorded against. A key
    // bound to no account here vouches for nobody.
    let Some(verifier) = crate::member_account_in_namespace(store, &group_id, &signer)? else {
        bail!(MembershipError::TeeVerifierNotMember);
    };
    policy_gate.require_tee_attestation_verifier_membership(&verifier)?;
    let policy = policy_gate.read_required_tee_admission_policy()?;
    policy_gate.validate_tee_attestation_allowlists(&policy, claims)?;

    // A TEE node an admin evicted stays evicted. Attestation proves the node is
    // running the expected measured stack — it says nothing about whether this
    // group still wants it, so it must not be able to launder away a removal.
    // Only an admin `MemberAdded` readmits them.
    //
    // A `Left` block does not stop re-admission: re-attesting is itself a fresh
    // authorization, unlike passively re-inheriting into an Open subgroup.
    if let Some(GroupExitReason::Removed) =
        ReentryRepository::new(store).block_of(&group_id, &member_account)?
    {
        bail!(MembershipError::RemovedFromGroup {
            group_id: format!("{group_id:?}"),
            identity: format!("{member:?}"),
        });
    }

    policy_gate.admit_member_if_absent(&member_account, role)?;
    // Not redundant with the deny-list retraction inside `add_member`:
    // `admit_member_if_absent` gates on the inheritance-aware `is_member`, so a
    // TEE that inherits membership from an ancestor — no direct row in this
    // group — skips the add entirely. A prior kick from THIS group deny-listed
    // them (the deny entry IS the removal when there is no row to delete), and
    // nothing else would clear it. Re-admission via attestation must.
    DenyListRepository::new(store).clear(&group_id, &member_account)?;

    // The admission is accepted, so bind the replica's device in the SAME apply
    // — the point of the cleartext form. `record_join_credential` checks the
    // credential belongs to `member` (the attested key), so a credential lifted
    // from another announcement binds nothing.
    super::member_joined_open::record_join_credential(ctx, member_account, account)?;

    ctx.queue_event(crate::op_events::OpEvent::TeeMemberAdmitted {
        group_id: group_id.to_bytes(),
        member: member_account,
    });
    if let Some(event) = crate::build_auto_follow_set_if_enabled(store, &group_id, &member_account)?
    {
        ctx.queue_event(event);
    }
    Ok(())
}
