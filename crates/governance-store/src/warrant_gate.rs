//! The at-cut half of admitting a delegated delta.
//!
//! # Why it is shaped this way
//!
//! `calimero_account` verifies a [`Delegation`] as a self-contained credential:
//! the warrant is signed by the device it names, and both named keys belong to
//! the accounts it names. That is authenticity, and it is history-independent by
//! design — two replicas with different folded state reach the same verdict.
//!
//! Everything that makes the credential *authoritative* needs a causal cut, and
//! lives here. Like the envelope branch it pairs with, it is **one function**
//! rather than a check per receive path: five paths each writing their own
//! version is five chances to get it right in four places, and the one that gets
//! it wrong accepts a write the others refuse — divergence, not a rejection.
//!
//! # What is deliberately NOT checked here
//!
//! **`Warrant::not_after`.** Wall-clock expiry must not gate an apply. Peers
//! apply at different times, so a warrant that expired between two receivers
//! would be accepted by one and refused by the other, and authorization would
//! stop converging. This is the same reason `calimero-account` has no
//! certificate expiry at all, recorded there as a deliberate absence. The bound
//! belongs where a single clock decides and nothing has converged yet: the relay
//! refusing a stale warrant at the API boundary, before it executes.
//!
//! Checking it here would look like defence in depth and would actually be a
//! convergence bug.

use calimero_account::{AccountId, Delegation, Warrant};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::ContextId;
use calimero_store::{key, types, Store};
use eyre::Result as EyreResult;

use crate::account_bindings::AccountBindingRepository;
use crate::capabilities::CapabilitiesRepository;
use crate::membership::MembershipPath;
use crate::MembershipRepository;

/// Why a delegated delta was refused at the cut.
///
/// Typed rather than a string because the five cases send an operator somewhere
/// different: a revoked device is an offboarding that worked, a missing
/// capability is an admin who has not granted it yet, and a spent nonce is a
/// relay replaying — which is the one worth alerting on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum WarrantRefusal {
    /// The context belongs to no group, so there is nothing to authorize against.
    #[error("context belongs to no group; a delegated write has no group to be authorized in")]
    NoOwningGroup,
    /// The author's device has been revoked in this group.
    #[error("the author's device has been revoked in this group")]
    AuthorDeviceRevoked,
    /// The executor's device has been revoked in this group.
    #[error("the executor's device has been revoked in this group")]
    ExecutorDeviceRevoked,
    /// The account the change is attributed to is not a member here.
    #[error("the author's account is not a member of the group owning this context")]
    AuthorNotAMember,
    /// The operator holds no authorship grant on the owning group.
    #[error("the executor holds no CAN_AUTHOR_ON_BEHALF grant on the group owning this context")]
    ExecutorMayNotAuthor,
    /// This warrant's nonce has already been spent, or is too old to judge.
    #[error("this warrant's nonce has already been spent by this author device")]
    NonceAlreadySpent,
}

/// Admit a delegated delta at the cut, spending its nonce.
///
/// Call this only with a [`Delegation`] whose envelope already verified — the
/// warrant reaching here is assumed authentic, because
/// `verify_delta_envelope` establishes that and this function would otherwise be
/// authorizing an unchecked claim.
///
/// **Read-only.** It answers "may this apply", including whether the nonce is
/// still spendable, and writes nothing. Spending is
/// [`spend_warrant_nonce`], deliberately separate — see below.
///
/// # Where this pair must be called, and why it is two functions
///
/// Spending a nonce is a write, which gives this an ordering constraint the
/// signature check does not have. Doing both in one call cannot satisfy it:
///
/// * Spend **before** the apply and a delta whose apply then fails has burned
///   the member's nonce for nothing. The retry presents the same warrant and
///   reads as a replay, so the write is lost permanently.
/// * Spend **after** the apply, in one call with the checks, and a delta the
///   checks refuse has already applied — unauthorized.
///
/// So: this runs before the apply and decides it, and `spend_warrant_nonce`
/// runs after the apply succeeded. Both must be under the same lock the apply
/// holds, or two concurrent deltas could each read the nonce as unspent —
/// `Store::apply` is writes-only with no read set, so a batch does not make a
/// read-modify-write atomic. The delta apply path already holds the DAG write
/// lock and the per-context execution lock across apply and commit, which is
/// exactly this ledger's key granularity.
///
/// A delta already known to the DAG must not reach either function: its
/// warrant's nonce is spent, so a re-delivery over gossip would be refused as a
/// replay of itself.
///
/// # Errors
/// [`WarrantRefusal`] for a delta that must not apply, or a store failure.
pub fn check_delegated_delta(
    store: &Store,
    context_id: &ContextId,
    delegation: &Delegation,
) -> EyreResult<()> {
    let warrant: &Warrant = &delegation.warrant;

    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Err(WarrantRefusal::NoOwningGroup.into());
    };

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(&group_id, delegation.author_proof.statement.device)? {
        return Err(WarrantRefusal::AuthorDeviceRevoked.into());
    }
    if bindings.is_revoked(&group_id, delegation.executor_proof.statement.device)? {
        return Err(WarrantRefusal::ExecutorDeviceRevoked.into());
    }

    // The author's ACCOUNT, not the device key: bindings are per group, and a
    // thin client's device never joins one. The certificate is what ties the key
    // to the account; this asks whether that account may write here.
    if MembershipRepository::new(store).check_path(&group_id, &warrant.author_account)?
        == MembershipPath::None
    {
        return Err(WarrantRefusal::AuthorNotAMember.into());
    }

    if !holds_authorship(store, &group_id, warrant.executor)? {
        return Err(WarrantRefusal::ExecutorMayNotAuthor.into());
    }

    let _admitted = next_nonce_state(store, context_id, warrant)?;
    Ok(())
}

/// Record this warrant's nonce as spent.
///
/// Call only after [`check_delegated_delta`] passed AND the delta applied, under
/// the same lock — see that function's docs for why the two are separate.
///
/// # Errors
/// [`WarrantRefusal::NonceAlreadySpent`] if the nonce was spent between the
/// check and here (which the shared lock is what prevents), or a store failure.
pub fn spend_warrant_nonce(
    store: &Store,
    context_id: &ContextId,
    delegation: &Delegation,
) -> EyreResult<()> {
    let warrant: &Warrant = &delegation.warrant;
    let next = next_nonce_state(store, context_id, warrant)?;
    let key = key::ContextWarrantNonce::new(*context_id, warrant.author_device_key);
    store.handle().put(&key, &next)?;
    Ok(())
}

/// Whether `account` holds the authorship grant on the group owning
/// `context_id`.
///
/// Public because the relay needs the same answer *before* it executes, not only
/// at apply: an intent for a context where it holds no grant must be refused at
/// the API, never executed and published. Peers would drop the result, and to
/// the member a silently dropped write is indistinguishable from data loss —
/// which then gets diagnosed as a client bug.
///
/// Read on the group that OWNS the context, falling back to that group's
/// membership anchor — see [`holds_authorship`] for why the fallback exists and
/// [`MemberCapabilities::CAN_AUTHOR_ON_BEHALF`] for why both reads are
/// deterministic across the peers that apply the delta.
///
/// # Errors
/// Propagates the store read failure. A context belonging to no group is not an
/// error, it is simply no grant.
pub fn account_may_author(
    store: &Store,
    context_id: &ContextId,
    account: AccountId,
) -> EyreResult<bool> {
    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    holds_authorship(store, &group_id, account)
}

/// Which group's capability row carries `account`'s authorship grant, if any.
///
/// **Reports where; [`account_may_author`] decides whether.** The two now read
/// the same source — this returns the group whose row carries the grant, and the
/// gate is that answer collapsed to a bool — so they cannot contradict each
/// other about the same relay. What the group buys a caller is the difference
/// between "granted here" and "granted once at the root for the whole fleet",
/// which is what a later revoke or narrow has to edit, and which a bare bool
/// cannot express.
///
/// Resolution order: the grant on `group_id` itself if the account is an
/// effective member holding it there, otherwise the row at the ancestor the
/// account inherits its membership through.
///
/// # Why it looks at this group and its anchor, and nothing else
///
/// It mirrors [`MembershipRepository::check_path`] exactly, which is the whole
/// design rule: **a grant reaches wherever membership reaches, and no further.**
/// `check_path` already stops at a non-Open boundary and already returns the
/// closest ancestor holding a direct row, so deferring to it means this cannot
/// report a grant across a privacy boundary the membership walk itself refuses
/// to cross — a subgroup that required its own admission also requires its own
/// grant. Re-deriving the traversal here would be a second implementation of
/// that rule, free to drift from the first.
///
/// An intermediate ancestor between `group` and `anchor` cannot hold a
/// meaningful row: capability rows are written alongside membership rows, and by
/// `check_path`'s definition the anchor is the closest ancestor that has one.
///
/// One conservative edge follows from that deferral. `check_path` short-circuits
/// on an inherited *admin*, returning the first Open ancestor the account
/// administers as the anchor without requiring a member row there — so an admin
/// of a mid-tree group whose authorship grant sits further up resolves to that
/// mid-tree anchor, finds no row, and is refused. Refusing is the safe
/// direction, and `CAN_AUTHOR_ON_BEHALF` is deliberately not implied by admin,
/// so an admin is not a special case that ought to pass regardless. Widening it
/// would mean climbing past the anchor, which is exactly the second
/// implementation of the traversal this defers in order to avoid.
pub fn authorship_grant_source(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<Option<ContextGroupId>> {
    let membership = MembershipRepository::new(store);

    // `effective_capabilities` rather than `check_path` + a raw row read, because
    // it is the deny-list-aware pair of the two. `check_path` deliberately does
    // NOT consult the deny-list, so building on it directly would report a grant
    // for a node kicked from an Open subgroup — where the deny entry *is* the
    // removal, there being no direct row to delete. Reusing the audited read
    // keeps that rule in one place instead of restating it here.
    //
    // `None` means not an effective member of this group by any path, so nothing
    // reachable from here can carry a grant.
    let Some(here) = membership.effective_capabilities(group_id, &account)? else {
        return Ok(None);
    };
    if MemberCapabilities::from_bits_truncate(here)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
    {
        return Ok(Some(*group_id));
    }

    // Not granted on this group. The anchor is the only other place it can live:
    // membership is inherited from there, and `check_path` has already refused to
    // cross any non-Open boundary on the way. A `Direct` member has no anchor, so
    // its own row above was the whole answer.
    let MembershipPath::Inherited { anchor, .. } = membership.check_path(group_id, &account)?
    else {
        return Ok(None);
    };
    let Some(bits) = CapabilitiesRepository::new(store).member_capability(&anchor, &account)?
    else {
        return Ok(None);
    };
    Ok(MemberCapabilities::from_bits_truncate(bits)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
        .then_some(anchor))
}

/// [`authorship_grant_source`] keyed by context, mirroring [`account_may_author`].
///
/// A context registered to no group reports `None` for the same reason the gate
/// refuses it: there is no group whose capabilities could carry a grant.
pub fn authorship_grant_source_for_context(
    store: &Store,
    context_id: &ContextId,
    account: AccountId,
) -> EyreResult<Option<ContextGroupId>> {
    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Ok(None);
    };
    authorship_grant_source(store, &group_id, account)
}

/// Whether the operator holds the authorship grant on `group_id`.
///
/// # A grant reaches wherever membership reaches, and no further
///
/// This is one line because [`authorship_grant_source`] already computes the
/// answer, and having the gate and the descriptor share it is the point: a
/// client that reads "granted on the namespace" can no longer be told "refused"
/// by the node that said it.
///
/// It used to read the capability row on `group_id` alone. Two things change.
///
/// **An ancestor grant now counts.** A TEE fleet node is admitted once at the
/// namespace root while contexts live in subgroups, and the capability does not
/// propagate down the tree — so a namespace-wide grant left every subgroup
/// context refused even though the node was an inherited member of the subgroup,
/// held its key, and could have run the method. That was the common shape, not a
/// corner.
///
/// **Membership is now required.** A bare capability row used to pass here,
/// with no membership check at all. It should not: a relay that is not an
/// effective member of the context's group has no business originating writes
/// in it. Today the writers mostly hold that line themselves —
/// `MemberCapabilitySet` bails unless the account is already a direct member,
/// and `remove_member` deletes the capability row alongside the member row — so
/// this closes a class rather than a known live hole: the non-atomic window in
/// that removal, and the next writer that forgets. What it buys structurally is
/// that the gate's answer is now one deny-list-aware predicate instead of two
/// that can drift apart.
///
/// The private-subgroup and deny-list properties are inherited from
/// [`authorship_grant_source`] rather than restated: a `Restricted` subgroup
/// required its own admission, so it still requires its own grant, and a node
/// deny-listed off an Open subgroup is refused there. So is the fallback's
/// boundary: it fires for an *inherited* member only, so a group that admitted
/// this node in its own right decides for itself. Both halves of that rule read
/// the same way — a group that required its own admission requires its own
/// grant.
///
/// # Peers must agree
///
/// Both directions are authorization evaluated **at the cut**, so a node running
/// this and a node running the old read would disagree about whether the same
/// delegated delta is authorized — and then hold different state. This has to
/// land as one coordinated upgrade, not a rolling one.
fn holds_authorship(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<bool> {
    Ok(authorship_grant_source(store, group_id, account)?.is_some())
}

/// The ledger state that would result from accepting this warrant's nonce, or
/// [`WarrantRefusal::NonceAlreadySpent`] if it may not be accepted.
///
/// A window rather than a high-water mark, because gossip gives no ordering
/// between two warrants from one device — see [`types::ContextWarrantNonce`].
fn next_nonce_state(
    store: &Store,
    context_id: &ContextId,
    warrant: &Warrant,
) -> EyreResult<types::ContextWarrantNonce> {
    let key = key::ContextWarrantNonce::new(*context_id, warrant.author_device_key);
    match store.handle().get(&key)? {
        Some(seen) => {
            let seen: types::ContextWarrantNonce = seen;
            Ok(seen
                .accept(warrant.nonce)
                .ok_or(WarrantRefusal::NonceAlreadySpent)?)
        }
        None => Ok(types::ContextWarrantNonce::first(warrant.nonce)),
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::Store;

    use super::{
        account_may_author, authorship_grant_source, check_delegated_delta, spend_warrant_nonce,
        WarrantRefusal,
    };
    use crate::test_fixtures::{
        enrol_member, nest_for_test, real_join_account, sample_meta_with_admin, test_store,
    };
    use crate::{CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository};
    use calimero_account::AccountId;
    use calimero_account::{Delegation, Warrant};
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::context::ContextId;

    const GROUP: [u8; 32] = [0xC0; 32];
    const CONTEXT: [u8; 32] = [0xC1; 32];
    const AUTHOR_KEY: [u8; 32] = [0x0A; 32];
    const RELAY_KEY: [u8; 32] = [0x0B; 32];

    struct World {
        store: Store,
        group: ContextGroupId,
        context: ContextId,
        delegation: Delegation,
    }

    /// A group with the author as a member and the relay holding authorship —
    /// the state in which a delegated write is supposed to be accepted.
    fn seed(nonce: u64) -> World {
        let store = test_store();
        let group = ContextGroupId::from(GROUP);
        let context = ContextId::from(CONTEXT);

        MetaRepository::new(&store)
            .save(
                &group,
                &sample_meta_with_admin(calimero_account::AccountId::from([0xEE; 32])),
            )
            .expect("save meta");
        crate::contexts::register_context_in_group(&store, &group, &context)
            .expect("register context");

        let author_pk = PublicKey::from(AUTHOR_KEY);
        let relay_pk = PublicKey::from(RELAY_KEY);
        let author = enrol_member(&store, &group, &author_pk);
        let relay = enrol_member(&store, &group, &relay_pk);

        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&group, &author, GroupMemberRole::Member)
            .expect("add the author");
        membership
            .add_member(&group, &relay, GroupMemberRole::Member)
            .expect("add the relay");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &group,
                &relay,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("grant authorship");

        let author_device_sk = PrivateKey::from(AUTHOR_KEY);
        let warrant = Warrant::sign(
            &author_device_sk,
            context,
            author,
            relay,
            Warrant::intent_hash("send_message", b"{}"),
            nonce,
            u64::MAX,
        )
        .expect("warrant must sign");

        let delegation = Delegation {
            warrant: Box::new(warrant),
            author_proof: real_join_account(&author_pk),
            executor_proof: real_join_account(&relay_pk),
            executor_key: relay_pk,
        };

        World {
            store,
            group,
            context,
            delegation,
        }
    }

    /// The accept direction, which every other test in this file assumes and
    /// none of them prove. A gate that refused everything would leave the
    /// refusal tests green and the feature entirely broken.
    #[test]
    fn a_well_formed_delegated_delta_is_admitted() {
        let w = seed(7);

        check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect("a member's write via an authorized relay must be admitted");
    }

    /// And the relay's grant is what makes it so — the same delta with the
    /// capability withdrawn must be refused, or the grant means nothing.
    #[test]
    fn the_same_delta_is_refused_once_authorship_is_withdrawn() {
        let w = seed(7);
        check_delegated_delta(&w.store, &w.context, &w.delegation).expect("precondition");

        let relay = w.delegation.warrant.executor;
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &relay, MemberCapabilities::empty().bits())
            .expect("withdraw authorship");

        let err = check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect_err("withdrawing the grant must refuse the write");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::ExecutorMayNotAuthor)
        );
    }

    /// Closed by default: a relay that was never granted anything is refused,
    /// so the absence of a row is not read as permission.
    #[test]
    fn a_relay_with_no_capability_row_may_not_author() {
        let w = seed(7);
        let other = ContextId::from([0xDD; 32]);

        assert!(
            account_may_author(&w.store, &w.context, w.delegation.warrant.executor)
                .expect("read the grant"),
            "precondition: the seeded relay holds the grant here"
        );
        assert!(
            !account_may_author(&w.store, &other, w.delegation.warrant.executor)
                .expect("read the grant"),
            "a context in no group must not be readable as a grant"
        );
    }

    /// **The narrowing.** A capability row with no membership behind it no
    /// longer authorizes. The gate used to read that row and nothing else, so
    /// the bit alone was permission.
    ///
    /// No writer produces this state on purpose — `MemberCapabilitySet` bails
    /// unless the account is already a direct member, and `remove_member`
    /// deletes the capability row with the member row. The state is reachable
    /// anyway: those deletes are three separate writes and explicitly not
    /// atomic, so a crash between them leaves exactly this until replay heals
    /// it. Which is the argument for the narrowing — the invariant is currently
    /// upheld by every writer agreeing to uphold it, and this makes the reader
    /// stop depending on that.
    ///
    /// Mutation check: restore the old `member_capability`-only body and this
    /// test fails while every other test in this file still passes.
    #[test]
    fn a_bare_capability_row_with_no_membership_may_not_author() {
        let store = test_store();
        let orphan = ContextGroupId::from([0x0E; 32]);
        let context = ContextId::from([0x0D; 32]);
        let stranger = AccountId::from([0x5B; 32]);

        MetaRepository::new(&store)
            .save(
                &orphan,
                &sample_meta_with_admin(AccountId::from([0xEE; 32])),
            )
            .expect("save meta");
        crate::contexts::register_context_in_group(&store, &orphan, &context)
            .expect("register context");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &orphan,
                &stranger,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("write the row directly, bypassing the op that guards membership");

        assert!(
            !MembershipRepository::new(&store)
                .is_member(&orphan, &stranger)
                .expect("read membership"),
            "precondition: the row exists and the membership does not"
        );
        assert!(
            !account_may_author(&store, &context, stranger).expect("read the gate"),
            "the bit is not permission on its own — a non-member must not \
             originate writes in the group's contexts"
        );
    }

    // ── `authorship_grant_source`: where a grant lives, and what it allows ──
    //
    // Every test below builds `namespace → subgroup`, puts the context in the
    // SUBGROUP, and grants only at the namespace. That is the shape the fleet
    // actually runs: a TEE node is admitted once at the namespace root, while
    // contexts live in subgroups (channels, DMs, per-team groups).
    //
    // The gate now resolves through the same helper, so these assert BOTH
    // layers: what the descriptor reports and what `account_may_author`
    // decides. That pairing is the change — the two could previously disagree
    // about the same relay — and the boundary tests are what keep the widening
    // from becoming a hole.

    /// A subgroup under `namespace`, Open so membership inherits, with `tee`
    /// admitted at the ROOT only and holding `CAN_JOIN_OPEN_SUBGROUPS` so the
    /// inheritance path is live. Returns `(namespace, subgroup, context, tee)`.
    fn nested(
        grant_at_root: bool,
    ) -> (Store, ContextGroupId, ContextGroupId, ContextId, AccountId) {
        let store = test_store();
        let namespace = ContextGroupId::from([0xB1; 32]);
        let subgroup = ContextGroupId::from([0xB2; 32]);
        let context = ContextId::from([0xB3; 32]);
        let tee = AccountId::from([0x7E; 32]);

        for gid in [namespace, subgroup] {
            MetaRepository::new(&store)
                .save(&gid, &sample_meta_with_admin(AccountId::from([0xEE; 32])))
                .expect("save meta");
        }
        nest_for_test(&store, &namespace, &subgroup);
        // The context is in the SUBGROUP — not the root. This is the whole point.
        crate::contexts::register_context_in_group(&store, &subgroup, &context)
            .expect("register context");

        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .expect("open the subgroup");

        // Admitted at the ROOT only, exactly as a fleet node is.
        MembershipRepository::new(&store)
            .add_member(&namespace, &tee, GroupMemberRole::ReadOnlyTee)
            .expect("admit at the root");

        let mut root_caps = MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS;
        if grant_at_root {
            root_caps |= MemberCapabilities::CAN_AUTHOR_ON_BEHALF;
        }
        CapabilitiesRepository::new(&store)
            .set_member_capability(&namespace, &tee, root_caps.bits())
            .expect("set the root mask");

        (store, namespace, subgroup, context, tee)
    }

    /// The shape the fleet runs: admitted once at the namespace root, context in
    /// a subgroup, granted only at the root — and it works.
    ///
    /// Before this change the descriptor reported the namespace while the gate
    /// refused, so a client was told where the grant was and then turned away by
    /// the node that told it. Now both read the same source.
    #[test]
    fn a_namespace_grant_reaches_a_subgroup_context() {
        let (store, namespace, subgroup, context, tee) = nested(true);

        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            Some(namespace),
            "the grant lives on the namespace and must be reported as such"
        );
        assert!(
            account_may_author(&store, &context, tee).expect("read the gate"),
            "the gate now honours the ancestor grant the descriptor reports, so the \
             two can no longer disagree about the same relay"
        );
    }

    /// Granted on the context's own group: reported as that group, and allowed.
    #[test]
    fn a_grant_on_the_contexts_own_group_is_reported_as_that_group() {
        let (store, _namespace, subgroup, context, tee) = nested(false);
        MembershipRepository::new(&store)
            .add_member(&subgroup, &tee, GroupMemberRole::ReadOnlyTee)
            .expect("admit directly");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &subgroup,
                &tee,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("grant here");

        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            Some(subgroup),
        );
        assert!(account_may_author(&store, &context, tee).expect("read the gate"));
    }

    /// **The boundary of the fallback.** A DIRECT member of the subgroup does
    /// not reach the ancestor grant — only an inherited one does.
    ///
    /// This is the rule, not an oversight, and it lines up with how admission
    /// works. A direct row is a per-group decision someone made, so the
    /// capability row beside it is that group's own statement about the node and
    /// must not be overridden from above. The fleet path never lands here: a TEE
    /// admission into an Open subgroup goes through `admit_member_if_absent`,
    /// which gates on the inheritance-aware `is_member` and so writes no row for
    /// a node that already inherits — it stays `Inherited`. A node that DID need
    /// its own admission is precisely the node that needs its own grant.
    ///
    /// It also follows from deferring to `check_path`, which short-circuits on a
    /// direct row and never computes an anchor to fall back to. Widening this
    /// would mean re-deriving the traversal.
    #[test]
    fn a_direct_member_of_the_subgroup_does_not_reach_the_ancestor_grant() {
        let (store, namespace, subgroup, context, tee) = nested(true);
        let membership = MembershipRepository::new(&store);

        assert!(
            account_may_author(&store, &context, tee).expect("read the gate"),
            "precondition: while inherited, the root grant reaches this context"
        );

        // Admitted in its own right, with nothing written for it here.
        membership
            .add_member(&subgroup, &tee, GroupMemberRole::ReadOnlyTee)
            .expect("admit directly");
        assert_eq!(
            membership
                .check_path(&subgroup, &tee)
                .expect("read the path"),
            crate::membership::MembershipPath::Direct,
            "precondition: the direct row is what changes the path"
        );

        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            None,
            "the root grant is still there, and is deliberately out of reach"
        );
        assert!(
            !account_may_author(&store, &context, tee).expect("read the gate"),
            "a group that admitted this node in its own right decides for itself"
        );
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .member_capability(&namespace, &tee)
                .expect("read the root row")
                .map(|b| b & MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits() != 0),
            Some(true),
            "and the root grant really is still written — this is scoping, not loss"
        );
    }

    /// Granted nowhere reachable: absent, not a stale ancestor.
    #[test]
    fn no_grant_anywhere_reports_nothing() {
        let (store, _namespace, subgroup, _context, tee) = nested(false);
        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            None,
            "CAN_JOIN_OPEN_SUBGROUPS alone is not an authorship grant"
        );
    }

    /// **The security property.** A non-Open subgroup terminates the membership
    /// walk, so a namespace grant must not be reported through it.
    ///
    /// A private subgroup required its own admission; it therefore requires its
    /// own grant. Reporting the ancestor here would tell a client the relay is
    /// "nearly" authorized for a group it is not even a member of — and would be
    /// the exact bug that widening the gate later must not introduce.
    #[test]
    fn a_namespace_grant_is_not_reported_across_a_private_subgroup_boundary() {
        let (store, _namespace, subgroup, context, tee) = nested(true);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Restricted)
            .expect("close the subgroup");

        assert!(
            !MembershipRepository::new(&store)
                .is_member(&subgroup, &tee)
                .expect("read membership"),
            "precondition: closing the subgroup ends the inheritance path"
        );
        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            None,
            "a grant must not be reported across a boundary membership cannot cross"
        );
        assert!(
            !account_may_author(&store, &context, tee).expect("read the gate"),
            "and the widened gate must not cross it either — a private subgroup \
             required its own admission, so it requires its own grant"
        );
    }

    /// Deny-listed on the subgroup: the inheritance is revoked there, so the
    /// ancestor grant stops being reachable too.
    ///
    /// Without this, a node kicked from a subgroup would still be reported as
    /// grant-carrying for it — and a kick from an Open subgroup IS the deny
    /// entry, since there is no direct row to delete.
    #[test]
    fn a_deny_listed_node_reports_no_grant_for_that_subgroup() {
        let (store, _namespace, subgroup, context, tee) = nested(true);
        assert!(
            account_may_author(&store, &context, tee).expect("read the gate"),
            "precondition: the ancestor grant reaches this context before the kick"
        );

        DenyListRepository::new(&store)
            .mark(&subgroup, &tee)
            .expect("deny-list on the subgroup");

        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            None,
        );
        assert!(
            !account_may_author(&store, &context, tee).expect("read the gate"),
            "a kick from an Open subgroup IS the deny entry, so it has to revoke \
             the inherited grant at the gate and not merely in the descriptor"
        );
    }

    /// A stray row on a group the account is no member of is not a grant there.
    #[test]
    fn a_row_on_a_group_with_no_membership_is_not_reported() {
        let store = test_store();
        let orphan = ContextGroupId::from([0x0F; 32]);
        let stranger = AccountId::from([0x5A; 32]);
        MetaRepository::new(&store)
            .save(
                &orphan,
                &sample_meta_with_admin(AccountId::from([0xEE; 32])),
            )
            .expect("save meta");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &orphan,
                &stranger,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("write a row without membership");

        assert_eq!(
            authorship_grant_source(&store, &orphan, stranger).expect("locate the grant"),
            None,
            "membership is checked first, so an orphaned row carries no grant"
        );
    }

    /// A group's default capabilities reach an attested TEE node on admission,
    /// so a fleet relay does not need a per-node grant.
    ///
    /// This is the ergonomic answer to "every fleet node lands
    /// authorship-closed": set the mask once on the namespace and every
    /// non-admin member admitted afterwards inherits it. It is asserted through
    /// `account_may_author` rather than by reading the capability row, because
    /// that function is what `POST .../intents` and the relay descriptor both
    /// call — a row that the gate does not read would prove nothing.
    ///
    /// The role matters: `ReadOnlyTee` is read-only for its OWN writes, and the
    /// read-only gate is only ever applied to a delta's author, never to its
    /// executor. So a read-only TEE node relaying for someone else is coherent,
    /// and this pins that it is also reachable.
    #[test]
    fn a_groups_default_capabilities_reach_an_admitted_tee_node() {
        let w = seed(7);
        let tee = calimero_account::AccountId::from([0x7E; 32]);

        CapabilitiesRepository::new(&w.store)
            .set_default_capabilities(&w.group, MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits())
            .expect("set the group default");

        // Admitted AFTER the default is set, and never granted anything
        // directly — the whole point is that no per-node op is needed.
        //
        // Through `admit_member_if_absent`, which is the call both TEE-attestation
        // apply handlers make (`ops/namespace/member_joined_via_tee.rs`,
        // `ops/group/member_joined_via_tee_attestation.rs`), rather than the
        // `add_member` it currently delegates to. Asserting the entry point means
        // a refactor that stops routing admission through `add_member` fails here
        // instead of passing while production regresses.
        crate::membership::MembershipPolicy::new(&w.store, w.group)
            .admit_member_if_absent(&tee, &GroupMemberRole::ReadOnlyTee)
            .expect("admit the TEE node");

        assert!(
            account_may_author(&w.store, &w.context, tee).expect("read the grant"),
            "a TEE node admitted under a default mask carrying CAN_AUTHOR_ON_BEHALF \
             must be able to relay without a per-node grant"
        );
    }

    /// The control for the test above: without the default, the same admission
    /// leaves the node closed.
    ///
    /// Without this, that test would pass just as happily if `add_member` were
    /// granting authorship to every TEE node regardless of the default — which
    /// is the failure it is meant to rule out, not demonstrate.
    #[test]
    fn an_admitted_tee_node_is_closed_when_no_default_is_set() {
        let w = seed(7);
        let tee = calimero_account::AccountId::from([0x7E; 32]);

        crate::membership::MembershipPolicy::new(&w.store, w.group)
            .admit_member_if_absent(&tee, &GroupMemberRole::ReadOnlyTee)
            .expect("admit the TEE node");

        assert!(
            !account_may_author(&w.store, &w.context, tee).expect("read the grant"),
            "admission alone must not confer authorship — it is implied by \
             neither membership nor the TEE role"
        );
    }

    /// The author must be a member. This is the check that would silently pass
    /// if it were keyed by device rather than by account — a thin client's
    /// device is in no group's rows.
    #[test]
    fn an_author_who_is_not_a_member_is_refused() {
        let w = seed(7);
        let author = w.delegation.warrant.author_account;
        MembershipRepository::new(&w.store)
            .remove_member(&w.group, &author)
            .expect("remove the author");

        let err = check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect_err("a non-member's write must be refused");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::AuthorNotAMember)
        );
    }

    /// The pair's contract: checking does not spend, so a delta refused after
    /// the check would not have burned the member's nonce.
    #[test]
    fn checking_does_not_spend_the_nonce() {
        let w = seed(7);

        check_delegated_delta(&w.store, &w.context, &w.delegation).expect("first check");
        check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect("a second check must still pass — checking is read-only");
    }

    /// And spending does, exactly once.
    #[test]
    fn spending_refuses_the_second_presentation_of_one_warrant() {
        let w = seed(7);

        check_delegated_delta(&w.store, &w.context, &w.delegation).expect("check");
        spend_warrant_nonce(&w.store, &w.context, &w.delegation).expect("first spend");

        let err = check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect_err("a spent warrant must not be admitted again");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::NonceAlreadySpent)
        );
    }

    /// A different warrant from the same author still applies — spending one
    /// nonce must not wall off the sequence.
    #[test]
    fn spending_one_nonce_does_not_block_the_next() {
        let first = seed(7);
        check_delegated_delta(&first.store, &first.context, &first.delegation).expect("check");
        spend_warrant_nonce(&first.store, &first.context, &first.delegation).expect("spend");

        // Same author, same relay, same store — a later warrant.
        let author_device_sk = PrivateKey::from(AUTHOR_KEY);
        let next_warrant = Warrant::sign(
            &author_device_sk,
            first.context,
            first.delegation.warrant.author_account,
            first.delegation.warrant.executor,
            Warrant::intent_hash("send_message", b"{\"n\":2}"),
            8,
            u64::MAX,
        )
        .expect("warrant must sign");
        let next = Delegation {
            warrant: Box::new(next_warrant),
            ..first.delegation.clone()
        };

        check_delegated_delta(&first.store, &first.context, &next)
            .expect("the next warrant in the sequence must still be admitted");
    }
}
