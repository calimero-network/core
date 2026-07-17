//! `GroupOp::MemberJoinedViaTeeAttestation` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::build_auto_follow_set_if_enabled;
use super::context::GroupApplyCtx;
use crate::membership::TeeAttestationClaims;
use crate::{DenyListRepository, MembershipError, ReentryRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupExitReason;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    claims: &TeeAttestationClaims<'_>,
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    if *role != GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::TeeRoleMustBeReadOnly);
    }
    ctx.membership_policy()
        .require_tee_attestation_verifier_membership(signer)?;
    let policy = ctx
        .membership_policy()
        .read_required_tee_admission_policy()?;
    ctx.membership_policy()
        .validate_tee_attestation_allowlists(&policy, claims)?;
    // A TEE node an admin evicted stays evicted. Attestation proves the node is
    // running the expected measured stack — it says nothing about whether this
    // group still wants it, so it must not be able to launder away a removal.
    // Only an admin `MemberAdded` readmits them.
    //
    // A `Left` block does not stop re-admission here: re-attesting is itself a
    // fresh authorization, unlike passively re-inheriting into an Open subgroup.
    if let Some(GroupExitReason::Removed) =
        ReentryRepository::new(store).block_of(group_id, member)?
    {
        bail!(MembershipError::RemovedFromGroup {
            group_id: format!("{group_id:?}"),
            identity: format!("{member:?}"),
        });
    }
    ctx.membership_policy()
        .admit_member_if_absent(member, role)?;
    // Not redundant with the deny-list retraction inside `add_member`:
    // `admit_member_if_absent` gates on the inheritance-aware `is_member`, so a
    // TEE that inherits membership from an ancestor — no direct row in this
    // group — skips the add entirely. A prior kick from THIS group deny-listed
    // them (the deny entry IS the removal when there is no row to delete), and
    // nothing else would clear it. Re-admission via attestation must.
    DenyListRepository::new(store).clear(group_id, member)?;
    ctx.queue_event(crate::op_events::OpEvent::TeeMemberAdmitted {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    // #2422 Option 2: TEE attestation goes through
    // `admit_member_if_absent` → `add_group_member`, which writes
    // the new default `{contexts: true, subgroups: false}`. The
    // fleet-join sidecar (`crates/server/src/admin/handlers/tee/
    // fleet_join.rs`) then issues an explicit `SetMemberAutoFollow
    // {true, true}` op, which fires its own `AutoFollowSet`. That
    // creates a second cascade — both join_context attempts are
    // idempotent (see auto_follow.rs:101-107), so the only cost
    // is two rate-limiter tokens. Documented and accepted.
    if let Some(event) = build_auto_follow_set_if_enabled(ctx.store(), ctx.group_id(), member)? {
        ctx.queue_event(event);
    }
    Ok(())
}
