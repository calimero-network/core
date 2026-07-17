//! Re-entry control: who may come back into a group after leaving it, and how.
//!
//! Exiting a group — by any route — writes a [`GroupReentryBlock`] for
//! `(group_id, identity)`. Nobody re-enters passively. What lifts the block
//! depends on how they left:
//!
//! | Exit | What lifts the block |
//! |---|---|
//! | `MemberRemoved` (an admin kicked them) | An admin `MemberAdded`, and nothing else. |
//! | `MemberLeft` (they walked out) | A fresh invitation they have not already used, or an admin `MemberAdded`. |
//!
//! In both cases they also stop flowing back into Open subgroups by
//! inheritance, which is otherwise automatic for any parent member holding
//! `CAN_JOIN_OPEN_SUBGROUPS` — and is precisely how a kicked member would
//! otherwise walk straight back into the subgroup they were kicked from.
//!
//! # Why this is not the deny-list
//!
//! The two look alike — both are per-`(group_id, identity)` marker rows written
//! on removal — but they answer different questions and have opposite
//! lifetimes, and collapsing them would break both:
//!
//! * The **deny-list** ([`crate::DenyListRepository`]) is a derived view of
//!   "not currently a member". It silences an identity's state deltas at the
//!   receive filter, and it is retracted the instant a member row is written —
//!   `MembershipRepository::add_member_with_keys` clears it at the choke point.
//! * The **re-entry block** is an authorization record that deliberately
//!   *survives* a member-row write, because its entire job is to make a re-join
//!   attempt fail. It is cleared only by the paths in the table above.
//!
//! So the block must never be cleared from `add_member_with_keys`: the sync
//! responder pre-registers joiners through that same call, and a removed member
//! opening a join stream would otherwise unban themselves.
//!
//! # Why invitation consumption is a set, not a timestamp
//!
//! "They can't reuse the invitation they left with" could be enforced by
//! stamping an `issued_at` on the invitation and comparing it against when they
//! exited. That would be O(1) state instead of a growing set — and it would put
//! a **wall clock inside folded governance state**. Nodes that disagree on the
//! clock would then disagree on *membership*, which is exactly the divergence
//! the governance DAG exists to prevent. (The responder's existing expiry check
//! gets away with a wall clock only because key delivery is point-to-point and
//! cannot diverge folded state.) Exact set membership has no clock in it and
//! cannot diverge, so consumption is recorded per
//! `(group_id, identity, invitation_nonce)`.
//!
//! Consumption is keyed by identity as well as nonce because an open invitation
//! is a bearer token with no invitee field: the same nonce legitimately admits
//! many different identities, which is what makes a shared join link work. What
//! must not happen is one identity replaying it after they exit.
//!
//! # Hash-neutral, like the deny-list
//!
//! Neither column feeds `compute_state_hash`. They do not need to: every node
//! applies the same ops from the same DAG in causal order, so the rows derive
//! identically everywhere. The state hash buys divergence *detection*, not
//! convergence — and hashing a column written during `MemberRemoved` apply
//! would force `compute_state_hash_after_remove` (the sign-time simulation that
//! predicts the post-removal hash) to model the block write too. Miss that and
//! every honest removal reports a false divergence and triggers a state
//! re-fetch. See the ordering invariant on the `MemberRemoved` handler.
//!
//! # What this does not stop
//!
//! It blocks an *identity*, not a person. Nothing prevents a removed member
//! from generating a fresh keypair and walking back in through an open
//! invitation as a stranger. Every identity-based ban has this property;
//! closing it needs invitations bound to a named invitee, which the current
//! bearer-token invitation model does not support.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupConsumedInvitation, GroupExitReason, GroupReentryBlock, GroupReentryBlockValue,
    GROUP_CONSUMED_INVITATION_PREFIX, GROUP_REENTRY_BLOCK_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::collect_keys_with_prefix;
use super::MembershipError;

/// Typed Repository for the re-entry block and the consumed-invitation set.
/// See the module docs for the policy these two columns implement together.
pub struct ReentryRepository<'a> {
    store: &'a Store,
}

impl<'a> ReentryRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record that `identity` exited `group_id`, and how.
    ///
    /// Idempotent, and last-write-wins on the reason: an identity who leaves and
    /// is then removed ends up `Removed` (the stricter block), which is the
    /// correct outcome — an admin's kick should not be softened by an earlier
    /// voluntary leave.
    ///
    /// **Caller contract:** invoke only from the apply path of the op that
    /// removed the member row, after the row is gone. The block is authorization
    /// state, so writing it while the identity is still a member would let a
    /// current member be locked out of a group they are in.
    pub fn block(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        reason: GroupExitReason,
    ) -> EyreResult<()> {
        let key = GroupReentryBlock::new(group_id.to_bytes(), *identity);
        let mut handle = self.store.handle();
        handle
            .put(&key, &GroupReentryBlockValue { reason })
            .map_err(|e| eyre::eyre!("ReentryRepository::block: {e}"))?;
        Ok(())
    }

    /// Lift the re-entry block for `identity` on `group_id`. Idempotent.
    ///
    /// Only two callers may do this: the `MemberAdded` apply handler (an admin
    /// re-adding them, which is the sole unban for a `Removed` block) and a
    /// successful invitation join that satisfies [`Self::require_invitation_admits`]
    /// (which readmits a `Left` block only). It must **not** be called from
    /// `MembershipRepository::add_member_with_keys` — see the module docs.
    pub fn clear_block(&self, group_id: &ContextGroupId, identity: &PublicKey) -> EyreResult<()> {
        let key = GroupReentryBlock::new(group_id.to_bytes(), *identity);
        let mut handle = self.store.handle();
        handle
            .delete(&key)
            .map_err(|e| eyre::eyre!("ReentryRepository::clear_block: {e}"))?;
        Ok(())
    }

    /// How `identity` exited `group_id`, or `None` if they have not exited it.
    pub fn block_of(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<Option<GroupExitReason>> {
        let key = GroupReentryBlock::new(group_id.to_bytes(), *identity);
        let handle = self.store.handle();
        Ok(handle
            .get(&key)
            .map_err(|e| eyre::eyre!("ReentryRepository::block_of: {e}"))?
            .map(|v: GroupReentryBlockValue| v.reason))
    }

    /// Record that `identity` has used the invitation identified by
    /// `invitation_nonce` to join `group_id`. Idempotent.
    pub fn mark_invitation_consumed(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        invitation_nonce: [u8; 32],
    ) -> EyreResult<()> {
        let key = GroupConsumedInvitation::new(group_id.to_bytes(), *identity, invitation_nonce);
        let mut handle = self.store.handle();
        handle
            .put(&key, &())
            .map_err(|e| eyre::eyre!("ReentryRepository::mark_invitation_consumed: {e}"))?;
        Ok(())
    }

    /// Whether `identity` has already used this invitation on `group_id`.
    pub fn is_invitation_consumed(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        invitation_nonce: [u8; 32],
    ) -> EyreResult<bool> {
        let key = GroupConsumedInvitation::new(group_id.to_bytes(), *identity, invitation_nonce);
        let handle = self.store.handle();
        handle
            .has(&key)
            .map_err(|e| eyre::eyre!("ReentryRepository::is_invitation_consumed: {e}"))
    }

    /// Gate an invitation-based join. Bails unless this invitation may readmit
    /// this identity to this group right now.
    ///
    /// Two ways it can fail: the identity was *removed* (no invitation readmits
    /// them — only an admin `MemberAdded` does), or they are replaying an
    /// invitation they have already consumed (they left, and that invitation is
    /// spent for them; they need a fresh one).
    ///
    /// A first-time joiner has neither a block nor a consumption row, so this is
    /// a no-op for them.
    pub fn require_invitation_admits(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        invitation_nonce: [u8; 32],
    ) -> EyreResult<()> {
        if let Some(GroupExitReason::Removed) = self.block_of(group_id, identity)? {
            bail!(MembershipError::RemovedFromGroup {
                group_id: format!("{group_id:?}"),
                identity: format!("{identity:?}"),
            });
        }
        if self.is_invitation_consumed(group_id, identity, invitation_nonce)? {
            bail!(MembershipError::InvitationAlreadyConsumed {
                group_id: format!("{group_id:?}"),
                identity: format!("{identity:?}"),
            });
        }
        Ok(())
    }

    /// Gate an inheritance-based join into an Open subgroup. Bails if the
    /// identity has exited this group by any route.
    ///
    /// Stricter than [`Self::require_invitation_admits`]: inheritance is passive
    /// and carries no fresh authorization, so *any* prior exit blocks it —
    /// a voluntary leaver included. Re-entry has to be an explicit act.
    pub fn require_inheritance_admits(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<()> {
        if self.block_of(group_id, identity)?.is_some() {
            bail!(MembershipError::ReentryBlocked {
                group_id: format!("{group_id:?}"),
                identity: format!("{identity:?}"),
            });
        }
        Ok(())
    }

    /// Drop every re-entry block and consumed-invitation row under `group_id`.
    /// Used during group teardown so neither set outlives the group it describes.
    pub fn clear_all_for_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();

        // Scan from the lexicographic minimum of the trailing key components —
        // RocksDB compares byte-wise, so a forward iterator seeded at all-zeroes
        // visits every row whose `group_id` matches. Same convention as
        // `DenyListRepository::clear_all_for_group`.
        let blocks = collect_keys_with_prefix(
            self.store,
            GroupReentryBlock::new(gid, PublicKey::from([0u8; 32])),
            GROUP_REENTRY_BLOCK_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let consumed = collect_keys_with_prefix(
            self.store,
            GroupConsumedInvitation::new(gid, PublicKey::from([0u8; 32]), [0u8; 32]),
            GROUP_CONSUMED_INVITATION_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let mut handle = self.store.handle();
        for key in blocks {
            handle
                .delete(&key)
                .map_err(|e| eyre::eyre!("ReentryRepository::clear_all_for_group: {e}"))?;
        }
        for key in consumed {
            handle
                .delete(&key)
                .map_err(|e| eyre::eyre!("ReentryRepository::clear_all_for_group: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};

    const NONCE_A: [u8; 32] = [0xA1; 32];
    const NONCE_B: [u8; 32] = [0xB2; 32];

    fn bob() -> PublicKey {
        PublicKey::from([0x02; 32])
    }

    #[test]
    fn no_block_and_no_consumption_for_a_first_time_joiner() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        assert!(repo.block_of(&gid, &bob()).unwrap().is_none());
        assert!(!repo.is_invitation_consumed(&gid, &bob(), NONCE_A).unwrap());
        repo.require_invitation_admits(&gid, &bob(), NONCE_A)
            .expect("a first-time joiner must be admitted");
        repo.require_inheritance_admits(&gid, &bob())
            .expect("a first-time inheritor must be admitted");
    }

    #[test]
    fn a_removed_identity_is_admitted_by_no_invitation_at_all() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        repo.block(&gid, &bob(), GroupExitReason::Removed).unwrap();

        // Not even a nonce they have never seen before — that is the whole
        // point of a removal outranking an invitation.
        assert!(repo
            .require_invitation_admits(&gid, &bob(), NONCE_B)
            .is_err());
        assert!(repo.require_inheritance_admits(&gid, &bob()).is_err());
    }

    #[test]
    fn a_leaver_needs_a_fresh_invitation_not_the_one_they_used() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        // Bob joined with NONCE_A, then left.
        repo.mark_invitation_consumed(&gid, &bob(), NONCE_A)
            .unwrap();
        repo.block(&gid, &bob(), GroupExitReason::Left).unwrap();

        assert!(
            repo.require_invitation_admits(&gid, &bob(), NONCE_A)
                .is_err(),
            "replaying the invitation he left with must not readmit him"
        );
        repo.require_invitation_admits(&gid, &bob(), NONCE_B)
            .expect("a freshly issued invitation must readmit a voluntary leaver");
    }

    #[test]
    fn a_leaver_does_not_flow_back_in_by_inheritance() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        repo.block(&gid, &bob(), GroupExitReason::Left).unwrap();

        assert!(
            repo.require_inheritance_admits(&gid, &bob()).is_err(),
            "inheritance is passive and carries no fresh authorization, so any \
             prior exit blocks it — a voluntary leaver included"
        );
    }

    #[test]
    fn consumption_is_per_identity_so_a_shared_invitation_still_works() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();
        let carol = PublicKey::from([0x03; 32]);

        // Bob burns the shared open-invite link and leaves.
        repo.mark_invitation_consumed(&gid, &bob(), NONCE_A)
            .unwrap();
        repo.block(&gid, &bob(), GroupExitReason::Left).unwrap();

        repo.require_invitation_admits(&gid, &carol, NONCE_A)
            .expect("the same open invitation must still admit a different identity");
    }

    #[test]
    fn clearing_the_block_readmits_but_leaves_consumption_recorded() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        repo.mark_invitation_consumed(&gid, &bob(), NONCE_A)
            .unwrap();
        repo.block(&gid, &bob(), GroupExitReason::Removed).unwrap();
        repo.clear_block(&gid, &bob()).unwrap();

        // An admin re-add lifts the ban...
        assert!(repo.block_of(&gid, &bob()).unwrap().is_none());
        repo.require_inheritance_admits(&gid, &bob()).unwrap();
        // ...but a spent invitation stays spent. Re-admission came from the
        // admin, not from the invitation, so the invitation is not un-burned.
        assert!(repo
            .require_invitation_admits(&gid, &bob(), NONCE_A)
            .is_err());
    }

    #[test]
    fn removal_outranks_an_earlier_voluntary_leave() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid = test_group_id();

        repo.block(&gid, &bob(), GroupExitReason::Left).unwrap();
        repo.block(&gid, &bob(), GroupExitReason::Removed).unwrap();

        assert_eq!(
            repo.block_of(&gid, &bob()).unwrap(),
            Some(GroupExitReason::Removed),
            "an admin's kick must not be softened by an earlier voluntary leave"
        );
        assert!(repo
            .require_invitation_admits(&gid, &bob(), NONCE_B)
            .is_err());
    }

    #[test]
    fn clear_all_for_group_wipes_both_columns_for_that_group_only() {
        let store = test_store();
        let repo = ReentryRepository::new(&store);
        let gid_a = test_group_id();
        let gid_b = ContextGroupId::from([0xBB; 32]);

        for gid in [&gid_a, &gid_b] {
            repo.block(gid, &bob(), GroupExitReason::Removed).unwrap();
            repo.mark_invitation_consumed(gid, &bob(), NONCE_A).unwrap();
        }

        repo.clear_all_for_group(&gid_a).unwrap();

        assert!(repo.block_of(&gid_a, &bob()).unwrap().is_none());
        assert!(!repo
            .is_invitation_consumed(&gid_a, &bob(), NONCE_A)
            .unwrap());
        assert_eq!(
            repo.block_of(&gid_b, &bob()).unwrap(),
            Some(GroupExitReason::Removed)
        );
        assert!(repo
            .is_invitation_consumed(&gid_b, &bob(), NONCE_A)
            .unwrap());
    }
}
