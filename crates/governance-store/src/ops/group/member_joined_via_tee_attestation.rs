//! `GroupOp::MemberJoinedViaTeeAttestation` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::build_auto_follow_set_if_enabled;
use super::context::GroupApplyCtx;
use crate::{DenyListRepository, MembershipError};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    mrtd: &str,
    rtmr0: &str,
    rtmr1: &str,
    rtmr2: &str,
    rtmr3: &str,
    tcb_status: &str,
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
        .validate_tee_attestation_allowlists(
            &policy, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, tcb_status,
        )?;
    ctx.membership_policy()
        .admit_member_if_absent(member, role)?;
    // Same rationale as `MemberAdded`: a TEE rejoining after a
    // prior removal should have their deny-list entry cleared.
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
