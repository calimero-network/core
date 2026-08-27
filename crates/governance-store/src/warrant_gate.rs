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
/// Read on the group that OWNS the context, which is deterministic for the
/// reason `CAN_AUTHOR_ON_BEHALF` documents: a peer applying a delta for a
/// context is by definition a member of the owning group and holds its key, so
/// every peer reads the same row and reaches the same verdict.
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

/// Whether the operator holds the authorship grant on `group_id`.
fn holds_authorship(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<bool> {
    let Some(bits) = CapabilitiesRepository::new(store).member_capability(group_id, &account)?
    else {
        // No row at all is no grant. Closed by default is the point.
        return Ok(false);
    };
    Ok(MemberCapabilities::from_bits_truncate(bits)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF))
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

    use super::{account_may_author, check_delegated_delta, spend_warrant_nonce, WarrantRefusal};
    use crate::test_fixtures::{
        enrol_member, real_join_account, sample_meta_with_admin, test_store,
    };
    use crate::{CapabilitiesRepository, MembershipRepository, MetaRepository};
    use calimero_account::{Delegation, Warrant};
    use calimero_context_config::types::ContextGroupId;
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
