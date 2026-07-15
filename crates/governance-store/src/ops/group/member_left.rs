//! `GroupOp::MemberLeft` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::super::super::verify_post_apply_state_hashes;
use super::context::GroupApplyCtx;
use crate::pending_rotation::group_rotates_on_departure;
use crate::{
    cascade_remove_member_from_group_tree, DenyListRepository, MembershipError, MembershipPolicy,
    MembershipRepository, MetaRepository, NamespaceRepository, PendingRotationRepository,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
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
    //
    // Captured here (not re-read after the mutation) so we can use the
    // role to gate the role-scoped `TeeMemberRemoved` follow-up event
    // emitted alongside `MemberRemoved` at the bottom of this function.
    let leaver_role = match MembershipRepository::new(store).role_of(group_id, member)? {
        Some(role) => role,
        None => bail!(MembershipError::MemberNotDirect(hex::encode(
            group_id.to_bytes()
        ))),
    };

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

        // Capture (descendant, role) per-direct-row so the role-scoped
        // `TeeMemberRemoved` follow-up event below can be gated
        // per-group. A leaver might be `Admin` at the namespace root
        // and `ReadOnlyTee` in some subgroup (or vice versa); only the
        // subgroups where the row was `ReadOnlyTee` should fire the
        // TEE event.
        let mut direct_descendants: Vec<(ContextGroupId, GroupMemberRole)> = Vec::new();
        for sub in &descendants {
            if let Some(role) = MembershipRepository::new(store).role_of(sub, member)? {
                if let Some(sub_meta) = MetaRepository::new(store).load(sub)? {
                    if sub_meta.owner_identity == *member {
                        bail!(MembershipError::OwnerOwnsSubgroup(hex::encode(
                            sub.to_bytes()
                        )));
                    }
                }
                let sub_policy = MembershipPolicy::new(store, *sub);
                sub_policy.ensure_not_last_admin_removal(member)?;
                direct_descendants.push((*sub, role));
            }
        }

        for (sub, role) in &direct_descendants {
            cascade_remove_member_from_group_tree(store, sub, member)?;
            MembershipRepository::new(store).remove_member(sub, member)?;
            // Self-leave cascade: deny-list every descendant
            // group where the leaver had a row, so their
            // state-delta traffic on those topics is dropped
            // until they re-join.
            DenyListRepository::new(store).mark(sub, member)?;
            // ...and record the forward-secrecy debt for each descendant that
            // encrypts under its own key. See the rotation note below.
            if group_rotates_on_departure(store, sub)? {
                PendingRotationRepository::new(store).mark(sub, member)?;
            }
            ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
                group_id: sub.to_bytes(),
                member: *member,
            });
            if *role == GroupMemberRole::ReadOnlyTee {
                ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
                    group_id: sub.to_bytes(),
                    member: *member,
                });
            }
        }
    }

    cascade_remove_member_from_group_tree(store, group_id, member)?;
    MembershipRepository::new(store).remove_member(group_id, member)?;
    // Deny-list the leaver on this group too. See
    // `MemberRemoved` for the same rationale.
    DenyListRepository::new(store).mark(group_id, member)?;

    // Forward secrecy on self-leave: record the debt, don't discharge it here.
    //
    // A key rotation is minted by whoever PUBLISHES the op that triggers it. For an
    // admin-initiated `MemberRemoved` that works — the publisher stays in the group.
    // Here the publisher IS the leaver, and they cannot rotate for themselves twice
    // over: they would have to mint the very key they are being cut off from (and
    // would keep it), and peers reject a rotation from a non-admin regardless. So the
    // leave and the rotation must be performed by DIFFERENT nodes.
    //
    // This row is the hand-off. It is written inside the deterministic, replicated
    // apply, so every node derives the same worklist with no coordination, and a
    // remaining admin discharges it by publishing `GroupKeyRotated` — which carries
    // the new key, wrapped for everyone who remains and for nobody who left.
    //
    // Any remaining admin may do it; there is deliberately no election. Two admins
    // racing mint different keys, and the keyring already converges on one (highest
    // epoch, ties broken by the larger key id — a total order, identical on every
    // node). Safety survives the race because EVERY competing key excludes the leaver.
    //
    // Only groups that encrypt under their own key are recorded — see
    // `group_rotates_on_departure`. Leaving the namespace root rotates the namespace
    // key itself, which is the only thing that stops a namespace-leaver from going on
    // reading the root and every Open subgroup beneath it.
    if group_rotates_on_departure(store, group_id)? {
        PendingRotationRepository::new(store).mark(group_id, member)?;
    }

    // Until that rotation lands, ops are still encrypted under the key the leaver
    // holds. The deny-list above stops them WRITING, and they unsubscribe, but a
    // leaver who keeps watching gossip can still read that window. It is bounded by
    // how quickly a remaining admin rotates, and it is observable (the pending row).
    //
    // Nothing here touches BACKWARD secrecy: the leaver keeps whatever they could
    // already decrypt. Old keys are never deleted from a keyring — rejoin and
    // re-keyshare depend on that. The guarantee is the same one removal gives:
    // decrypt everything up to and including your own departure, and nothing after.
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
    ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    // Role-scoped follow-up for the root-group removal. See the
    // matching block in `member_removed.rs` for rationale.
    if leaver_role == GroupMemberRole::ReadOnlyTee {
        ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
            group_id: group_id.to_bytes(),
            member: *member,
        });
    }
    Ok(())
}
