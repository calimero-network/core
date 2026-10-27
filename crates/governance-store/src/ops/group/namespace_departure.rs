//! The per-descendant teardown shared by a namespace-root leave and a namespace-root removal.

use super::context::GroupApplyCtx;
use crate::pending_rotation::group_rotates_on_departure;
use crate::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipRepository,
    MetaRepository, NamespaceRepository, PendingRotationRepository, ReentryRepository,
};
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::GroupExitReason;
use eyre::Result as EyreResult;

/// Takes `member` out of every descendant of the namespace root `root`, blocking
/// re-entry with `reason`.
///
/// Runs before the root row is torn down so `verify_post_apply_state_hashes` sees
/// the root's own rows change last. Per-descendant writes are not transactional: a
/// store error aborts the apply and the idempotent cascade finishes on re-apply.
pub(super) fn cascade_to_descendants(
    ctx: &mut GroupApplyCtx<'_>,
    root: &ContextGroupId,
    member: &AccountId,
    reason: GroupExitReason,
) -> EyreResult<()> {
    let store = ctx.store();
    let membership = MembershipRepository::new(store);
    for sub in &NamespaceRepository::new(store).collect_descendants(root)? {
        // Never touch the root's own rows here: they change after this loop.
        if sub == root {
            continue;
        }
        // Read before any write so the teardown gate does not depend on what the
        // cascade below touches.
        let direct_role = membership.role_of(sub, member)?;
        // Runs for inherited-only descendants too: they hold `ContextIdentity`
        // rows from auto-following contexts, and the purge is hash-neutral.
        cascade_remove_member_from_group_tree(store, sub, member)?;
        // Only a direct row has anything further to evict; deny-listing an
        // inherited one would strand an entry no subgroup-level re-join clears.
        let Some(role) = direct_role else {
            continue;
        };
        membership.remove_member(sub, member)?;
        DenyListRepository::new(store).mark(sub, member)?;
        if group_rotates_on_departure(store, sub)? {
            PendingRotationRepository::new(store).mark(sub, member)?;
        }
        ReentryRepository::new(store).block(sub, member, reason)?;
        ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
            group_id: sub.to_bytes(),
            member: *member,
        });
        if role.is_tee() {
            ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
                group_id: sub.to_bytes(),
                member: *member,
            });
        }
    }
    Ok(())
}

/// Whether `member` holds an `Admin` row, or the creator slot, in any descendant of `root`.
pub(super) fn administers_a_descendant(
    ctx: &GroupApplyCtx<'_>,
    root: &ContextGroupId,
    member: &AccountId,
) -> EyreResult<bool> {
    let store = ctx.store();
    let membership = MembershipRepository::new(store);
    for sub in &NamespaceRepository::new(store).collect_descendants(root)? {
        if membership.is_direct_admin(sub, member)?
            || MetaRepository::new(store)
                .load(sub)?
                .is_some_and(|meta| meta.admin_identity == *member)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
