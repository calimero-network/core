//! `GroupOp::TeeAuthorityEvidence` apply handler.
//!
//! The evidence proves itself, so any member may publish it. What apply checks
//! is the proof: the quote verifies offline against the collateral it carries,
//! binds the key it names, and satisfies the namespace's admission policy. A
//! failure refuses the op, so it never reaches the log.

use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::{MembershipError, NamespaceRepository};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &AccountId,
    attested_key: &PublicKey,
    quote: &[u8],
    collateral: Option<&[u8]>,
    attested_at: u64,
) -> EyreResult<()> {
    // The reader resolves to the root, so a subgroup copy would be dead data.
    if NamespaceRepository::new(ctx.store())
        .parent(ctx.group_id())?
        .is_some()
    {
        bail!("TeeAuthorityEvidence is namespace-scoped; it must be published on the root");
    }
    let Some(signer) = ctx.signer_account()? else {
        bail!(MembershipError::TeeVerifierNotMember);
    };
    ctx.membership_policy()
        .require_tee_attestation_verifier_membership(&signer)?;

    let verdict =
        crate::tee::verify_authority_evidence(attested_key, quote, collateral, attested_at)?;
    let policy = ctx
        .membership_policy()
        .read_required_tee_admission_policy()?;
    if verdict.is_mock && !policy.accept_mock {
        bail!("TeeAuthorityEvidence carries a mock quote the admission policy does not accept");
    }
    ctx.membership_policy()
        .validate_tee_attestation_allowlists(
            &policy,
            &crate::membership::TeeAttestationClaims {
                mrtd: &verdict.mrtd,
                rtmr0: &verdict.rtmr0,
                rtmr1: &verdict.rtmr1,
                rtmr2: &verdict.rtmr2,
                rtmr3: &verdict.rtmr3,
                tcb_status: &verdict.tcb_status,
            },
        )?;
    // Whether `attested_key` speaks for `member` is checked when the evidence
    // is read, not here: the binding may land after this op, and the reader
    // must refuse relabelled evidence either way.
    let _ = member;
    Ok(())
}
