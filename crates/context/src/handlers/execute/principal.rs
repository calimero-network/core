//! Who an execution runs as, held apart from whose key signs it.
//!
//! Execution used to take a single `executor: PublicKey` that answered two
//! unrelated questions: which identity the application observes, and which key
//! signs what the run produces. They collapsed into one because signing needs a
//! locally-held key, so the observed identity could only ever be an identity
//! this node holds a private key for.
//!
//! That collapse has a documented cost. The server's execute path resolves the
//! executor to the node's own identity and says so: an application enforcing
//! per-member permissions sees the node rather than the caller, so a member
//! whose in-application permissions are lower than the node owner's executes at
//! the higher level.
//!
//! # Why a type rather than two parameters
//!
//! The two values are already adjacent in every signature that carries them,
//! and adjacent same-shaped parameters are how they got swapped in the first
//! place — `AccountId` and `PublicKey` are both 32 bytes, and the compiler has
//! nothing to say about an argument list that passes them in the wrong order.
//! Naming the pair makes the ordering a field access instead of a position.
//!
//! # What this deliberately does not do
//!
//! It does not let a caller assert who it is. `Principal` is built inside the
//! trusted boundary from the authenticated caller and the target context, never
//! deserialized from a request. That invariant predates this type — it is why
//! the account was resolved inside the execute handler rather than passed down
//! from the RPC layer — and it survives unchanged here.
//!
//! The principal and the signer DO differ, on exactly one path: a delegated
//! execution reads both halves off the warrant
//! (`Principal::new(warrant.author_account, warrant.author_device_key)`), while
//! the envelope is signed by this node's own key. Every other construction site
//! still derives both from the node's identity.
//!
//! That divergence had a precondition, and it is why it waited for delegated
//! authorship rather than landing on its own: the sync path refuses a `User` leaf
//! whose owner is not its author's account — `user_leaf_author_is_its_owner` in
//! `calimero-node`. Both halves coming from the same warrant is what satisfies
//! it. Take the account from the warrant and the device from the node and the
//! leaf's owner stops matching its author: the change applies on the gossip path
//! and is dropped on hash-comparison, which is silent divergence rather than a
//! partial feature.

use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;

/// The identity an execution runs as.
///
/// Both halves are needed because they are consumed by different layers, and
/// neither substitutes for the other:
///
/// * `account` is the authorization subject. Governance rows are account-keyed
///   — membership, capabilities, writer sets, ownership stamps — and it is what
///   the guest observes through `env::executor_id()`, deliberately, so that an
///   application doing ownership counts one person once rather than once per
///   installation.
/// * `device` is the replica. Per-writer counter slots, the logical-clock seed
///   and the delta's `author_id` all key off it, so two devices of one account
///   are two replicas and must stay distinguishable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Principal {
    /// The account this run is authorized as and attributed to.
    pub account: AccountId,
    /// The device signing key identifying which replica is writing.
    pub device: PublicKey,
}

impl Principal {
    /// Bind an account to the device acting for it.
    pub(crate) const fn new(account: AccountId, device: PublicKey) -> Self {
        Self { account, device }
    }
}
