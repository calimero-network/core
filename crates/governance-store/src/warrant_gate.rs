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
use calimero_primitives::context::{ContextId, GroupMemberRole};
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
    /// The account the change is attributed to holds a read-only role
    /// (`ReadOnly`, or a TEE role) in the group owning the context, so it may
    /// not write there — through a relay any more than directly.
    #[error("the author's role in this context is read-only")]
    AuthorIsReadOnly,
    /// The operator holds no authorship grant on the owning group.
    #[error("the executor holds no CAN_AUTHOR_ON_BEHALF grant on the group owning this context")]
    ExecutorMayNotAuthor,
    /// The operator is a TEE replica. Relaying is a property of the role, not
    /// of a capability bit: only a `RelayTee` relays for a member, and a
    /// `ReadOnlyTee` never does, whatever its capability row says.
    #[error(
        "the executor is a TEE replica (ReadOnlyTee) and does not relay writes; the namespace \
         must admit relays with mode=relay"
    )]
    ExecutorIsTeeReplica,
    /// The operator is a plain `ReadOnly` member. Relaying writes a delta, so a
    /// read-only role never relays, whatever its capability row says — the same
    /// rule as a TEE replica, for a node admitted by invitation.
    #[error(
        "the executor's role in this context is read-only (ReadOnly), so it does not relay writes"
    )]
    ExecutorIsReadOnly,
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
    //
    // `effective_role` rather than a bare `check_path`: it is deny-list aware,
    // so an author kicked from an Open subgroup (where the deny entry IS the
    // removal) is not a member there, and it carries the role the read-only
    // check below needs — including a role inherited from an ancestor, which a
    // direct-row read would miss.
    let Some((author_role, _)) =
        MembershipRepository::new(store).effective_role(&group_id, &warrant.author_account)?
    else {
        return Err(WarrantRefusal::AuthorNotAMember.into());
    };
    // The read-only rule belongs to the AUTHOR: the write is theirs, and a
    // relay is not a way round a role that may not write. Refused here, before
    // anything executes, rather than executed and then discarded.
    if author_role.is_read_only() {
        return Err(WarrantRefusal::AuthorIsReadOnly.into());
    }

    if let Err(refusal) = executor_standing(store, &group_id, warrant.executor)? {
        return Err(refusal.into());
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

/// Whether `account` may author a member's write in `context_id`.
///
/// Public because the relay needs the same answer *before* it executes, not only
/// at apply: an intent for a context where it may not author must be refused at
/// the API, never executed and published. Peers would drop the result, and to
/// the member a silently dropped write is indistinguishable from data loss —
/// which then gets diagnosed as a client bug.
///
/// The rule is [`executor_standing`]'s: a `RelayTee` by its role, a
/// `ReadOnlyTee` never, anyone else by a `CAN_AUTHOR_ON_BEHALF` grant.
///
/// # Errors
/// Propagates the store read failure. A context belonging to no group is not an
/// error, it is simply no grant.
pub fn account_may_author(
    store: &Store,
    context_id: &ContextId,
    account: AccountId,
) -> EyreResult<bool> {
    Ok(executor_refusal_for_context(store, context_id, account)?.is_none())
}

/// Why `account` may not author a member's write in `context_id`, or `None`
/// when it may.
///
/// [`account_may_author`] with the reason kept, so `POST .../intents` can tell
/// a TEE replica apart from a node that is simply missing its grant — the two
/// send an operator to different places (the namespace's admission mode, or an
/// admin's capability grant).
///
/// # Errors
/// Propagates the store read failure.
pub fn executor_refusal_for_context(
    store: &Store,
    context_id: &ContextId,
    account: AccountId,
) -> EyreResult<Option<WarrantRefusal>> {
    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Ok(Some(WarrantRefusal::NoOwningGroup));
    };
    Ok(executor_standing(store, &group_id, account)?.err())
}

/// Which group carries `account`'s authority to author a member's write here,
/// if any.
///
/// **Reports where; [`account_may_author`] decides whether.** Both read
/// [`executor_standing`], so they cannot contradict each other about the same
/// relay. For a `RelayTee` the group is the one whose row carries the role (the
/// namespace root for a fleet relay); for anyone else it is the group whose
/// capability row carries `CAN_AUTHOR_ON_BEHALF` — see
/// [`capability_grant_source`]. A `ReadOnlyTee` reports `None` whatever its
/// capability row says.
pub fn authorship_grant_source(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<Option<ContextGroupId>> {
    Ok(executor_standing(store, group_id, account)?.ok())
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

/// Whether `account` may relay a member's write in `group_id`, and on whose
/// authority: `Ok(group)` names the group carrying it, `Err` says why not.
///
/// # Relaying comes from the role for a TEE, from a grant for anyone else
///
/// The executor's EFFECTIVE role decides first — the direct row, or the anchor
/// row a member inherits through, which is the shape a fleet node admitted once
/// at the namespace root has in every subgroup context:
///
/// * **`RelayTee`** relays by its role. Attestation under a namespace policy
///   whose mode is `relay` is the grant, so no `CAN_AUTHOR_ON_BEHALF` bit is
///   needed — a namespace whose default mask omits the bit still has working
///   relays.
/// * **`ReadOnlyTee`** never relays, even holding `CAN_AUTHOR_ON_BEHALF` from a
///   default mask or an explicit grant. It is the TEE replica: new namespaces
///   put the bit in their default mask, so a bit-based rule made every replica
///   admitted after that a relay by accident.
/// * **`Admin`, `Member`, `ReadOnly`** — self-hosted nodes run with
///   `--delegated-access` — keep the capability rule unchanged.
///
/// A member of no group reaches no role and is refused.
///
/// # Peers must agree
///
/// Authorization evaluated **at the cut**, so a node running this and a node
/// running an older rule would disagree about whether the same delegated delta
/// is authorized — and then hold different state. This lands as one
/// coordinated upgrade (`SIGNED_NAMESPACE_OP_SCHEMA_VERSION` 12), not a rolling
/// one.
fn executor_standing(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<Result<ContextGroupId, WarrantRefusal>> {
    let Some((role, role_group)) =
        MembershipRepository::new(store).effective_role(group_id, &account)?
    else {
        return Ok(Err(WarrantRefusal::ExecutorMayNotAuthor));
    };
    Ok(match role {
        GroupMemberRole::RelayTee => Ok(role_group),
        GroupMemberRole::ReadOnlyTee => Err(WarrantRefusal::ExecutorIsTeeReplica),
        GroupMemberRole::ReadOnly => Err(WarrantRefusal::ExecutorIsReadOnly),
        GroupMemberRole::Admin | GroupMemberRole::Member => {
            capability_grant_source(store, group_id, account)?
                .ok_or(WarrantRefusal::ExecutorMayNotAuthor)
        }
    })
}

/// Which group's capability row carries `account`'s `CAN_AUTHOR_ON_BEHALF`
/// grant, if any — the rule for a non-TEE executor.
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
///
/// **An ancestor grant counts, and membership is required.** A namespace-wide
/// grant reaches a subgroup context the account inherits into, and a bare
/// capability row with no membership behind it grants nothing. Both halves
/// read the same way — a group that required its own admission requires its
/// own grant — and the deny-list property is inherited from
/// `effective_capabilities`: a node deny-listed off an Open subgroup is refused
/// there.
fn capability_grant_source(
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
        account_may_author, authorship_grant_source, check_delegated_delta,
        executor_refusal_for_context, spend_warrant_nonce, WarrantRefusal,
    };
    use crate::test_fixtures::{
        enrol_member, nest_for_test, real_join_account, sample_meta_with_admin, test_store,
    };
    use crate::{CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository};
    use calimero_account::AccountId;
    use calimero_account::{Delegation, Warrant, WarrantTerms};
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::application::ApplicationId;
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
            WarrantTerms {
                context,
                author_account: author,
                executor: relay,
                app_version: ApplicationId::from([0u8; 32]),
                method: "send_message".to_owned(),
                intent_hash: Warrant::intent_hash("send_message", b"{}"),
                account_heads: vec![],
                governance_floor: vec![],
                nonce,
                not_after: u64::MAX,
            },
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
    // SUBGROUP, and grants only at the namespace. That is the shape a relay
    // actually runs in: admitted once at the namespace root, while contexts
    // live in subgroups (channels, DMs, per-team groups).
    //
    // The relay here is a `Member` — a self-hosted `--delegated-access` node —
    // because these pin the CAPABILITY rule. A TEE relay's authority comes from
    // its role instead, and the TEE tests further down pin that.
    //
    // The gate now resolves through the same helper, so these assert BOTH
    // layers: what the descriptor reports and what `account_may_author`
    // decides. That pairing is the change — the two could previously disagree
    // about the same relay — and the boundary tests are what keep the widening
    // from becoming a hole.

    /// A subgroup under `namespace`, Open so membership inherits, with a
    /// `Member` relay admitted at the ROOT only and holding `CAN_JOIN_OPEN_SUBGROUPS` so the
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
            .add_member(&namespace, &tee, GroupMemberRole::Member)
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
            .add_member(&subgroup, &tee, GroupMemberRole::Member)
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
            .add_member(&subgroup, &tee, GroupMemberRole::Member)
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

    // ── TEE executors: relaying comes from the role, not from a bit ──

    /// A delegation authored by the seeded author, executed by `relay_pk`.
    fn delegation_via(w: &World, relay_pk: PublicKey, relay: AccountId) -> Delegation {
        let warrant = Warrant::sign(
            &PrivateKey::from(AUTHOR_KEY),
            WarrantTerms {
                context: w.context,
                author_account: w.delegation.warrant.author_account,
                executor: relay,
                app_version: ApplicationId::from([0u8; 32]),
                method: "send_message".to_owned(),
                intent_hash: Warrant::intent_hash("send_message", b"{}"),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 7,
                not_after: u64::MAX,
            },
        )
        .expect("warrant must sign");
        Delegation {
            warrant: Box::new(warrant),
            author_proof: real_join_account(&PublicKey::from(AUTHOR_KEY)),
            executor_proof: real_join_account(&relay_pk),
            executor_key: relay_pk,
        }
    }

    /// **Regression (R1).** A TEE replica admitted under a default mask carrying
    /// `CAN_AUTHOR_ON_BEHALF` — which every new namespace's mask does — may not
    /// relay.
    ///
    /// Before: the gate read only the bit, so this passed, `POST .../intents`
    /// executed the write, and the execute path then discarded it as a
    /// read-only member's while answering `200` (context in the root) — or
    /// executed and published it (context in a subgroup, where the replica has
    /// no direct row and so did not read as read-only).
    ///
    /// Admitted through `admit_member_if_absent`, the call both TEE-attestation
    /// apply handlers make, so a refactor of the admission entry point cannot
    /// leave this passing while production regresses.
    #[test]
    fn a_tee_replica_may_not_relay_even_under_a_default_mask_granting_authorship() {
        let w = seed(7);
        let tee_pk = PublicKey::from([0x7E; 32]);
        let tee = enrol_member(&w.store, &w.group, &tee_pk);

        CapabilitiesRepository::new(&w.store)
            .set_default_capabilities(&w.group, MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits())
            .expect("set the group default");
        crate::membership::MembershipPolicy::new(&w.store, w.group)
            .admit_member_if_absent(&tee, &GroupMemberRole::ReadOnlyTee)
            .expect("admit the TEE node");
        assert_eq!(
            CapabilitiesRepository::new(&w.store)
                .member_capability(&w.group, &tee)
                .expect("read the row")
                .map(|bits| bits & MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits() != 0),
            Some(true),
            "precondition: the replica holds the bit, from the default mask"
        );

        assert!(!account_may_author(&w.store, &w.context, tee).expect("read the gate"));
        assert_eq!(
            authorship_grant_source(&w.store, &w.group, tee).expect("locate the grant"),
            None,
            "the descriptor must not report a grant the gate refuses"
        );
        assert_eq!(
            executor_refusal_for_context(&w.store, &w.context, tee).expect("read the gate"),
            Some(WarrantRefusal::ExecutorIsTeeReplica)
        );
        let err = check_delegated_delta(&w.store, &w.context, &delegation_via(&w, tee_pk, tee))
            .expect_err("peers must refuse a replica-relayed delta at the cut");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::ExecutorIsTeeReplica)
        );
    }

    /// **Regression (R2).** The fleet shape: a replica admitted once at the
    /// root with an explicit root grant, relaying into a subgroup context. The
    /// replica has no direct row there, which is why the execute path used to
    /// execute AND publish its relayed write rather than discard it.
    #[test]
    fn a_tee_replica_may_not_relay_into_a_subgroup_even_with_an_explicit_grant() {
        let (store, namespace, subgroup, context, tee) = nested(true);
        MembershipRepository::new(&store)
            .set_role(&namespace, &tee, GroupMemberRole::ReadOnlyTee)
            .expect("make the root row a replica");

        assert_eq!(
            MembershipRepository::new(&store)
                .effective_role(&subgroup, &tee)
                .expect("resolve the role"),
            Some((GroupMemberRole::ReadOnlyTee, namespace)),
            "precondition: the subgroup sees the replica through its root row"
        );
        assert!(!account_may_author(&store, &context, tee).expect("read the gate"));
        assert_eq!(
            executor_refusal_for_context(&store, &context, tee).expect("read the gate"),
            Some(WarrantRefusal::ExecutorIsTeeReplica)
        );
    }

    /// A plain `ReadOnly` member never relays either, even holding an explicit
    /// `CAN_AUTHOR_ON_BEHALF` grant: relaying writes a delta, so the rule is the
    /// replica's, by role. Both where the row is direct and where the subgroup
    /// sees it through the root, which is the shape the old gate let through.
    #[test]
    fn a_read_only_member_may_not_relay_even_with_an_explicit_grant() {
        let (store, namespace, subgroup, context, relay) = nested(true);
        MembershipRepository::new(&store)
            .set_role(&namespace, &relay, GroupMemberRole::ReadOnly)
            .expect("make the root row read-only");

        assert_eq!(
            MembershipRepository::new(&store)
                .effective_role(&subgroup, &relay)
                .expect("resolve the role"),
            Some((GroupMemberRole::ReadOnly, namespace)),
            "precondition: the subgroup sees the read-only role through its root row"
        );
        assert!(!account_may_author(&store, &context, relay).expect("read the gate"));
        assert_eq!(
            authorship_grant_source(&store, &subgroup, relay).expect("locate the grant"),
            None,
            "the descriptor must not report a grant the gate refuses"
        );
        assert_eq!(
            executor_refusal_for_context(&store, &context, relay).expect("read the gate"),
            Some(WarrantRefusal::ExecutorIsReadOnly)
        );
    }

    /// A TEE relay authors by its role, with no `CAN_AUTHOR_ON_BEHALF` bit
    /// anywhere — a namespace that strips the bit from its default mask still
    /// has working relays — and the descriptor reports the group whose row
    /// carries the role.
    #[test]
    fn a_tee_relay_authors_by_its_role_without_the_capability_bit() {
        let (store, namespace, subgroup, context, tee) = nested(false);
        MembershipRepository::new(&store)
            .set_role(&namespace, &tee, GroupMemberRole::RelayTee)
            .expect("make the root row a relay");

        assert!(account_may_author(&store, &context, tee).expect("read the gate"));
        assert_eq!(
            authorship_grant_source(&store, &subgroup, tee).expect("locate the grant"),
            Some(namespace),
        );
    }

    /// And the delta a relay produced is admitted at the cut, so `/intents`,
    /// the descriptor and every peer give one answer.
    #[test]
    fn a_tee_relayed_delta_is_admitted_at_the_cut() {
        let w = seed(7);
        let tee_pk = PublicKey::from([0x7E; 32]);
        let tee = enrol_member(&w.store, &w.group, &tee_pk);
        crate::membership::MembershipPolicy::new(&w.store, w.group)
            .admit_member_if_absent(&tee, &GroupMemberRole::RelayTee)
            .expect("admit the relay");

        check_delegated_delta(&w.store, &w.context, &delegation_via(&w, tee_pk, tee))
            .expect("a relay's delegated delta must be admitted");
    }

    /// A relay kicked from an Open subgroup no longer relays there: the deny
    /// entry is the removal, and the role resolution honours it.
    #[test]
    fn a_deny_listed_tee_relay_may_not_relay_into_that_subgroup() {
        let (store, namespace, subgroup, context, tee) = nested(false);
        MembershipRepository::new(&store)
            .set_role(&namespace, &tee, GroupMemberRole::RelayTee)
            .expect("make the root row a relay");
        DenyListRepository::new(&store)
            .mark(&subgroup, &tee)
            .expect("deny-list on the subgroup");

        assert_eq!(
            executor_refusal_for_context(&store, &context, tee).expect("read the gate"),
            Some(WarrantRefusal::ExecutorMayNotAuthor)
        );
    }

    /// Attestation alone does not make a replica a relay, with or without the
    /// default mask: the control for the regression above.
    #[test]
    fn an_admitted_tee_replica_is_closed_when_no_default_is_set() {
        let w = seed(7);
        let tee = calimero_account::AccountId::from([0x7E; 32]);

        crate::membership::MembershipPolicy::new(&w.store, w.group)
            .admit_member_if_absent(&tee, &GroupMemberRole::ReadOnlyTee)
            .expect("admit the TEE node");

        assert!(
            !account_may_author(&w.store, &w.context, tee).expect("read the grant"),
            "admission alone must not confer authorship"
        );
    }

    /// **Regression (R3).** A delegated write whose author is `ReadOnly` in the
    /// context is refused before it runs.
    ///
    /// Before: nothing on the delegated path asked the author's role. The relay
    /// executed and committed the write locally and published it; a peer
    /// holding the author's device binding then dropped it as a read-only
    /// member's, so the relay and its peers diverged, while a peer without the
    /// binding (a thin client's device) applied it.
    #[test]
    fn a_read_only_authors_delegated_write_is_refused() {
        let w = seed(7);
        MembershipRepository::new(&w.store)
            .set_role(
                &w.group,
                &w.delegation.warrant.author_account,
                GroupMemberRole::ReadOnly,
            )
            .expect("demote the author");

        let err = check_delegated_delta(&w.store, &w.context, &w.delegation)
            .expect_err("a read-only author may not write through a relay");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::AuthorIsReadOnly)
        );
    }

    /// The author's role is read where it is EFFECTIVE: a `ReadOnly` root
    /// member inheriting into an Open subgroup is read-only in its contexts,
    /// although it has no direct row there.
    #[test]
    fn an_inherited_read_only_author_is_refused_in_a_subgroup_context() {
        let (store, namespace, subgroup, context, relay) = nested(true);
        let author_pk = PublicKey::from(AUTHOR_KEY);
        let author = enrol_member(&store, &namespace, &author_pk);
        MembershipRepository::new(&store)
            .add_member(&namespace, &author, GroupMemberRole::ReadOnly)
            .expect("admit the author at the root");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &namespace,
                &author,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .expect("let the author inherit");
        assert!(MembershipRepository::new(&store)
            .is_member(&subgroup, &author)
            .expect("read membership"));

        let relay_pk = PublicKey::from(RELAY_KEY);
        let warrant = Warrant::sign(
            &PrivateKey::from(AUTHOR_KEY),
            WarrantTerms {
                context,
                author_account: author,
                executor: relay,
                app_version: ApplicationId::from([0u8; 32]),
                method: "send_message".to_owned(),
                intent_hash: Warrant::intent_hash("send_message", b"{}"),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after: u64::MAX,
            },
        )
        .expect("warrant must sign");
        let delegation = Delegation {
            warrant: Box::new(warrant),
            author_proof: real_join_account(&author_pk),
            executor_proof: real_join_account(&relay_pk),
            executor_key: relay_pk,
        };

        let err = check_delegated_delta(&store, &context, &delegation)
            .expect_err("an inherited read-only author may not write through a relay");
        assert_eq!(
            err.downcast_ref::<WarrantRefusal>(),
            Some(&WarrantRefusal::AuthorIsReadOnly)
        );
    }

    /// No refusal message may mention a nonce unless it is about one: relay
    /// clients treat a message containing "nonce" as a retryable replay.
    #[test]
    fn only_the_replay_refusal_mentions_a_nonce() {
        for refusal in [
            WarrantRefusal::NoOwningGroup,
            WarrantRefusal::AuthorDeviceRevoked,
            WarrantRefusal::ExecutorDeviceRevoked,
            WarrantRefusal::AuthorNotAMember,
            WarrantRefusal::AuthorIsReadOnly,
            WarrantRefusal::ExecutorMayNotAuthor,
            WarrantRefusal::ExecutorIsTeeReplica,
            WarrantRefusal::ExecutorIsReadOnly,
        ] {
            assert!(
                !refusal.to_string().contains("nonce"),
                "{refusal:?} reads as a replay: {refusal}"
            );
        }
        assert!(WarrantRefusal::NonceAlreadySpent
            .to_string()
            .contains("nonce"));
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
            WarrantTerms {
                context: first.context,
                author_account: first.delegation.warrant.author_account,
                executor: first.delegation.warrant.executor,
                app_version: ApplicationId::from([0u8; 32]),
                method: "send_message".to_owned(),
                intent_hash: Warrant::intent_hash("send_message", b"{\"n\":2}"),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 8,
                not_after: u64::MAX,
            },
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
