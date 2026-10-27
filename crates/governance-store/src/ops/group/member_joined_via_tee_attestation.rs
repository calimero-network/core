//! `GroupOp::MemberJoinedViaTeeAttestation` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::build_auto_follow_set_if_enabled;
use super::context::GroupApplyCtx;
use crate::membership::TeeAttestationClaims;
use crate::{DenyListRepository, MembershipError, MembershipRepository};
use calimero_account::AccountId;
use calimero_primitives::context::GroupMemberRole;
use calimero_store::key::GroupExitReason;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &AccountId,
    claims: &TeeAttestationClaims<'_>,
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let _signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // The verifier vouched for the attestation, so its key is resolved to the
    // account whose authority is checked. A key bound to no account here
    // vouches for nobody.
    // The root never has a parent, so only a subgroup's target needs the tree.
    if crate::NamespaceRepository::new(store)
        .parent(group_id)?
        .is_some()
    {
        crate::private_space::Tree::at_cut(
            store,
            ctx.authorizer(),
            group_id,
            ctx.cut(),
            crate::TreeReads {
                chains: &[*group_id],
                subtrees: &[],
            },
        )?
        .require_member_row_target(group_id)?;
    }
    let Some(verifier) = ctx.signer_account()? else {
        bail!(MembershipError::TeeVerifierNotAuthorized);
    };
    ctx.membership_policy()
        .require_tee_attestation_verifier(&verifier)?;
    let policy = ctx
        .membership_policy()
        .read_required_tee_admission_policy()?;
    ctx.membership_policy()
        .validate_tee_attestation_allowlists(&policy, claims)?;
    // The role is the policy's, not the admitter's: `ReadOnlyTee` in replica
    // mode, `RelayTee` in relay mode.
    ctx.membership_policy()
        .require_policy_tee_role(&policy, role)?;
    // A TEE node an admin evicted stays evicted. Attestation proves the node is
    // running the expected measured stack — it says nothing about whether this
    // group still wants it, so it must not be able to launder away a removal.
    // Only an admin `MemberAdded` readmits them.
    //
    // A `Left` block does not stop re-admission here: re-attesting is itself a
    // fresh authorization, unlike passively re-inheriting into an Open subgroup.
    if let Some(GroupExitReason::Removed) = ctx.exit_at_cut(member)? {
        bail!(MembershipError::RemovedFromGroup {
            group_id: group_id.to_string(),
            identity: member.to_string(),
        });
    }
    // A re-attestation of a node the cut seats writes no row a concurrent removal took.
    if ctx.direct_role(member)?.is_some()
        && MembershipRepository::new(store)
            .role_of(group_id, member)?
            .is_none()
    {
        return Ok(());
    }
    // Below the root a seat needs a workspace row at this op's cut, or it is not written.
    if crate::NamespaceRepository::new(store)
        .parent(group_id)?
        .is_some()
        && !crate::private_space::holds_workspace_seat_at_cut(
            store,
            ctx.authorizer(),
            group_id,
            ctx.cut(),
            member,
        )?
    {
        return Ok(());
    }
    ctx.membership_policy()
        .admit_or_convert_tee_member(member, role)?;
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
