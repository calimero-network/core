//! `GroupOp::MemberLeft` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::verify_post_apply_state_hashes;
use super::context::GroupApplyCtx;
use crate::group_store::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipError, MembershipPolicy,
    MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    expected_group_state_hash: &[u8; 32],
    expected_context_state_hashes: &[(ContextId, [u8; 32])],
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Self-leave: signer must equal the member being removed.
    // No capability check beyond self-equality — any member can
    // leave themselves without admin involvement.
    if signer != member {
        bail!(MembershipError::SelfLeaveOnly);
    }

    // Direct-row check. If `signer` is only an inherited member
    // (Open subgroup with no stored row), there's nothing to delete
    // here — they have to leave whichever ancestor anchors their
    // membership instead.
    if MembershipRepository::new(store)
        .role_of(group_id, member)?
        .is_none()
    {
        bail!(MembershipError::MemberNotDirect(hex::encode(
            group_id.to_bytes()
        )));
    }

    // Owner cannot self-leave. Must TransferOwnership first.
    if let Some(meta) = MetaRepository::new(store).load(group_id)? {
        if meta.owner_identity == *member {
            bail!(MembershipError::OwnerCannotSelfLeave(hex::encode(
                group_id.to_bytes()
            )));
        }
    }

    // Last-admin protection — same helper used by MemberRemoved.
    ctx.membership_policy()
        .ensure_not_last_admin_removal(member)?;

    // Detect namespace-leave: if this group has no parent it IS the
    // namespace, and leaving must cascade through every descendant
    // group where the leaver has a direct row. Per the design's
    // "no cascade for leave_group" rule, non-namespace groups don't
    // touch siblings or descendants. See § 6 for cascade semantics.
    let is_namespace_leave = NamespaceRepository::new(store).resolve(group_id)? == *group_id;

    if is_namespace_leave {
        // Walk subtree, gather descendants where leaver has a direct
        // row. Run owner + last-admin checks across all of them
        // BEFORE mutating anything, so a failure surfaces the
        // offending scope to the user with no half-applied cleanup.
        let descendants = NamespaceRepository::new(store).collect_descendants(group_id)?;

        let mut direct_descendants: Vec<ContextGroupId> = Vec::new();
        for sub in &descendants {
            if MembershipRepository::new(store)
                .role_of(sub, member)?
                .is_some()
            {
                if let Some(sub_meta) = MetaRepository::new(store).load(sub)? {
                    if sub_meta.owner_identity == *member {
                        bail!(MembershipError::OwnerOwnsSubgroup(hex::encode(
                            sub.to_bytes()
                        )));
                    }
                }
                let sub_policy = MembershipPolicy::new(store, *sub);
                sub_policy.ensure_not_last_admin_removal(member)?;
                direct_descendants.push(*sub);
            }
        }

        for sub in &direct_descendants {
            cascade_remove_member_from_group_tree(store, sub, member)?;
            MembershipRepository::new(store).remove_member(sub, member)?;
            // Self-leave cascade: deny-list every descendant
            // group where the leaver had a row, so their
            // state-delta traffic on those topics is dropped
            // until they re-join.
            DenyListRepository::new(store).mark(sub, member)?;
            crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
                group_id: sub.to_bytes(),
                member: *member,
            });
        }
    }

    cascade_remove_member_from_group_tree(store, group_id, member)?;
    MembershipRepository::new(store).remove_member(group_id, member)?;
    // Deny-list the leaver on this group too. See
    // `MemberRemoved` for the same rationale.
    DenyListRepository::new(store).mark(group_id, member)?;

    // NOTE on forward secrecy: this op deliberately does NOT trigger
    // the key-rotation pipeline that `MemberRemoved` does, because
    // the publisher (the leaver) cannot generate the new key without
    // also retaining it — which would defeat forward secrecy.
    // Proper forward secrecy on self-leave requires a follow-up
    // two-phase rotation (a remaining admin's apply hook publishes
    // KeyDelivery), which is tracked as a follow-up to this PR. For
    // now, an admin-initiated `MemberRemoved` is the path to a
    // cryptographically-complete leave; `MemberLeft` is the
    // governance-level departure (membership row removed, peers
    // observe the leave) without the rotation. Same caveat applies
    // to the namespace cascade above — row-removal only.
    //
    // Ordering invariant (mirrors `MemberRemoved`'s call site):
    // `verify_post_apply_state_hashes` must run after the last
    // mutation that touches `GroupMeta` or `GroupMember` rows
    // for `group_id`. The namespace-leave cascade above operates
    // on DESCENDANT group ids — those mutations don't change
    // `compute_group_state_hash(group_id)`'s inputs (the hash
    // only reads members of THIS group, not descendants). The
    // `remove_group_member(store, group_id, member)` call just
    // above is the only mutation here that affects the hash;
    // `cascade_remove_member_from_group_tree` touches
    // `ContextIdentity` rows and `mark_denied` touches
    // `GroupDeniedMember` rows, both in separate columns. If a
    // future mutation between `remove_group_member` and this
    // call DOES touch `GroupMeta` or `GroupMember` rows for
    // `group_id`, the recomputed hash will diverge from the
    // signer's pre-apply simulation on every honest receiver.
    ctx.divergence = verify_post_apply_state_hashes(
        store,
        group_id,
        "MemberLeft",
        expected_group_state_hash,
        expected_context_state_hashes,
    );
    crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    Ok(())
}
