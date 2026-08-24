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

use calimero_account::{Delegation, Warrant};
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
/// **Spends the nonce as a side effect, and only on success.** Every other check
/// runs first so a delta refused for revocation or a missing grant does not burn
/// the member's nonce — otherwise a relay could consume a member's whole
/// sequence by publishing writes it knew would be refused.
///
/// # Where this must be called, and it is not where the envelope check goes
///
/// Spending a nonce is a write, so this has an ordering constraint the
/// signature check does not:
///
/// * **After dedup.** A delta already in the DAG must never reach here. Its
///   warrant's nonce is spent, so a second pass would refuse a delta this node
///   has already applied — and on the gossip path a delta legitimately arrives
///   more than once.
/// * **Atomically with the apply.** If the nonce is spent and the apply then
///   fails, the member's write is lost permanently: the retry presents the same
///   warrant and is refused as a replay. The two writes belong in one batch.
///
/// Both point at the same place: the apply path, beside the row write — not the
/// pre-decrypt envelope check, which runs before either condition holds.
///
/// # Errors
/// [`WarrantRefusal`] for a delta that must not apply, or a store failure.
pub fn admit_delegated_delta(
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

    if !executor_may_author(store, &group_id, warrant)? {
        return Err(WarrantRefusal::ExecutorMayNotAuthor.into());
    }

    spend_nonce(store, context_id, warrant)
}

/// Whether the operator holds the authorship grant on `group_id`.
///
/// Read on the group that owns the context, which is deterministic here for the
/// reason `CAN_AUTHOR_ON_BEHALF` documents: a peer applying a delta for a context
/// is by definition a member of the owning group and holds its key, so every
/// peer can read the row and every peer agrees.
fn executor_may_author(
    store: &Store,
    group_id: &ContextGroupId,
    warrant: &Warrant,
) -> EyreResult<bool> {
    let Some(bits) =
        CapabilitiesRepository::new(store).member_capability(group_id, &warrant.executor)?
    else {
        // No row at all is no grant. Closed by default is the point.
        return Ok(false);
    };
    Ok(MemberCapabilities::from_bits_truncate(bits)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF))
}

/// Record this warrant's nonce as spent, refusing a repeat.
///
/// The window rather than a high-water mark, because gossip gives no ordering
/// between two warrants from one device — see [`types::ContextWarrantNonce`].
fn spend_nonce(store: &Store, context_id: &ContextId, warrant: &Warrant) -> EyreResult<()> {
    let key = key::ContextWarrantNonce::new(*context_id, warrant.author_device_key);
    let mut handle = store.handle();

    let next = match handle.get(&key)? {
        Some(seen) => {
            let seen: types::ContextWarrantNonce = seen;
            seen.accept(warrant.nonce)
                .ok_or(WarrantRefusal::NonceAlreadySpent)?
        }
        None => types::ContextWarrantNonce::first(warrant.nonce),
    };

    handle.put(&key, &next)?;
    Ok(())
}
