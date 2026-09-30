//! `GroupOp::FoundingRelayAttested` apply handler: the relay a member founded a
//! namespace through admits itself as the namespace's first TEE.
//!
//! A namespace founded through a relay has no admin node to admit a TEE and no
//! TEE to vouch for one, so it could never admit any. The delegated genesis
//! records the relay the founder named; only that relay may publish this, and
//! only once. Its quote is verified here, offline and the same way on every
//! peer, bound to the key that signed the op — so peers do not take the relay's
//! word that it is a TEE. The op is itself the namespace's first admission
//! policy (read back by `tee::read_tee_admission_policy`): signed releases of
//! the relay's own profile, `UpToDate`, relay mode. Further fleet TEEs are then
//! admitted the ordinary way, with this relay as a verifier.

use calimero_primitives::context::GroupMemberRole;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::tee::{read_tee_admission_policy, TeeAdmissionPolicyRead};
use crate::{MembershipRepository, NamespaceFoundingRepository, NamespaceRepository};

/// The TCB status a founding relay's quote must carry, and the policy admits.
pub(crate) const FOUNDING_TCB_STATUS: &str = "UpToDate";

#[expect(
    clippy::too_many_arguments,
    reason = "one argument per field of the op being applied"
)]
pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    account: &calimero_context_client::local_governance::JoinAccountCredential,
    quote: &[u8],
    collateral: Option<&[u8]>,
    attested_at: u64,
    release_version: &str,
    profile: &str,
    mock: bool,
) -> EyreResult<()> {
    let store = ctx.store();
    let group_id = *ctx.group_id();
    if NamespaceRepository::new(store).parent(&group_id)?.is_some() {
        bail!("FoundingRelayAttested is namespace-scoped; it must be published on the root");
    }

    let founding = NamespaceFoundingRepository::new(store);
    let Some((relay, attested)) = founding.founding_relay(&group_id)? else {
        bail!("this namespace was not founded through a relay, so it has no founding relay");
    };
    if attested {
        bail!("the founding relay has already attested in this namespace");
    }
    // The credential the op carries must be the relay's, for the key that
    // signed it. Resolved from the credential rather than the checker: the
    // relay's binding is folded by no projected op, so a checker at the cut
    // would park an op every live check admits.
    if !crate::ops::namespace::member_joined_open::join_op_proves_ownership(
        ctx.signer(),
        &relay,
        account,
    ) {
        bail!(
            "FoundingRelayAttested must be signed by the founding relay, with its own credential"
        );
    }

    if profile.trim().is_empty() || release_version.trim().is_empty() {
        bail!("FoundingRelayAttested must name the release and profile the relay runs");
    }
    let verdict =
        crate::tee::verify_founding_evidence(ctx.signer(), quote, collateral, attested_at)?;
    if verdict.is_mock != mock {
        bail!("FoundingRelayAttested says mock={mock} but its quote says otherwise");
    }
    if !verdict.is_mock && verdict.tcb_status != FOUNDING_TCB_STATUS {
        bail!(
            "the founding relay's TCB status is {}, not {FOUNDING_TCB_STATUS}",
            verdict.tcb_status
        );
    }
    // This op IS the first policy: it may not override one somebody set.
    if !matches!(
        read_tee_admission_policy(store, &group_id)?,
        TeeAdmissionPolicyRead::NotSet
    ) {
        bail!("this namespace already has a TEE admission policy");
    }

    MembershipRepository::new(store).set_role(&group_id, &relay, GroupMemberRole::RelayTee)?;
    founding.mark_founding_relay_attested(&group_id)?;
    ctx.queue_event(crate::op_events::OpEvent::TeeMemberAdmitted {
        group_id: group_id.to_bytes(),
        member: relay,
    });
    Ok(())
}
