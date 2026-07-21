//! `GroupOp::MemberAdded` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::build_auto_follow_set_if_enabled;
use super::super::super::contexts::restore_member_context_identities;
use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository, ReentryRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    if *role == GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::ReadOnlyTeeViaAttestationOnly);
    }
    ctx.permissions()
        .require_manage_members(signer, "add member")?;
    ctx.permissions().require_admin_to_add_admin(signer, role)?;
    // `add_member` also retracts any deny-list entry for the pair, so re-adding
    // a previously removed member transparently restores their network-level
    // access. The clear is a property of writing the member row now, not of this
    // handler — see `MembershipRepository::add_member_with_keys`.
    MembershipRepository::new(store).add_member(group_id, member, role.clone())?;
    // Lift the re-entry block. An admin re-adding someone is the ONLY thing that
    // readmits an identity an admin removed — no invitation does that, however
    // freshly issued.
    //
    // This lives here, in the admin-gated op handler, and deliberately NOT in
    // `add_member` alongside the deny-list clear. Several paths write a member
    // row without an admin authorizing it — the sync responder pre-registers a
    // joiner, `join_group` adds the joiner locally — and if the block were
    // cleared at that choke point, a removed member could unban themselves just
    // by opening a join stream. Reaching this line means `require_manage_members`
    // above has already passed, which is exactly the authority an unban needs.
    ReentryRepository::new(store).clear_block(group_id, member)?;
    // Restore per-context `ContextIdentity` rows that
    // `cascade_remove_member_from_group_tree` deleted on a prior
    // `MemberRemoved`. The local-rejoiner anti-spoof gate is
    // enforced inside `restore_member_context_identities` — on
    // peers (admin or other members applying this op) it is a
    // no-op. Idempotent on first-time adds: the joiner's later
    // `join_context` sees an existing row and skips.
    restore_member_context_identities(store, group_id, member)?;
    ctx.queue_event(crate::op_events::OpEvent::MemberAdded {
        group_id: group_id.to_bytes(),
        member: *member,
        role: role.clone(),
    });
    // #2422 Option 2: synthesize an `AutoFollowSet` event whenever
    // a freshly-written member row has `auto_follow.contexts` set
    // (the post-#2422 default). The auto-follow handler subscribes
    // to `AutoFollowSet` (not `MemberAdded`), so without this the
    // joiner would correctly auto-follow FUTURE
    // `OpEvent::ContextRegistered` events but never backfill
    // contexts that already existed in the group at join time —
    // which is the user-reported regression (Ronit/Fran 2026-05-20).
    // The handler short-circuits via `NotForSelf` on every node
    // except the joiner, so the cascade only fires once per
    // membership change.
    if let Some(event) = build_auto_follow_set_if_enabled(ctx.store(), ctx.group_id(), member)? {
        ctx.queue_event(event);
    }
    Ok(())
}
