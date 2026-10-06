//! The one shape every delegated statement shares: what a warrant is *about*
//! ([`WarrantScope`]), the fields every warrant carries ([`WarrantStatement`]),
//! and the bundle that carries one to a verifier ([`Delegated`]).
//!
//! # Why it is shaped this way
//!
//! **There are three warrants and one idea.** [`Warrant`] (a data intent),
//! [`GovernanceWarrant`] (one governance op) and [`ContextCreationWarrant`] (one
//! new context) are each "this author consents to this executor doing this one
//! thing, once". They differ only in what *this one thing* is, and that is the
//! scope. Everything else — the author account and device, the executor, the
//! nonce, the expiry, the cited heads — is the same field with the same meaning
//! in all three, and every verifier asks the same questions of it.
//!
//! So the statement kinds stay three types, and what they share is one trait.
//! [`WarrantStatement::scope`] is what a verifier switches on; every other
//! accessor is a question a verifier asks of any warrant without caring which.
//!
//! **Why the three are not merged into one wire type.** They could be — one
//! struct carrying a `WarrantScope` enum — and a new protocol would do exactly
//! that. Here it would change the bytes every client signs: each statement has
//! its own signing domain and preimage, pinned by the wire fixtures in
//! `src/tests/*_wire_fixture.rs` and re-derived by the JavaScript and Python
//! SDKs, and a node that verifies the new bytes cannot verify warrants minted by
//! a client that has not upgraded. What a merged type would buy — one verifier,
//! one set of standing checks, one nonce path — this module and the
//! governance-store's `warrant_admission` get without touching a byte.
//!
//! **The bundle is one generic type, and that is invisible on the wire.**
//! [`Delegation`], [`GovernanceDelegation`] and [`ContextCreationDelegation`]
//! were three structs with the same four fields and the same `verify`, written
//! out three times. Borsh encodes a struct as its fields in order and has no
//! notion of a type name, so `Delegated<Warrant>` serializes exactly as the old
//! `Delegation` did; the aliases keep every existing name, and the wire
//! fixtures are what prove the claim rather than this comment.
//!
//! [`Warrant`]: crate::Warrant
//! [`GovernanceWarrant`]: crate::GovernanceWarrant
//! [`ContextCreationWarrant`]: crate::ContextCreationWarrant
//! [`Delegation`]: crate::Delegation
//! [`GovernanceDelegation`]: crate::GovernanceDelegation
//! [`ContextCreationDelegation`]: crate::ContextCreationDelegation

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{AccountId, PublicKey};

use crate::device::DeviceCert;
use crate::error::AccountError;
use crate::governance::GovernanceOpKind;
use crate::signed::{AccountProof, Verified};

/// What a warrant authorizes its executor to act on.
///
/// The one field the three statement kinds do not share, typed so a verifier
/// matches on it rather than on which struct it happens to hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarrantScope {
    /// One intent in a context that exists — a [`crate::Warrant`].
    Context(ContextId),
    /// One governance op published on a group's log (the namespace's, for a
    /// root op) — a [`crate::GovernanceWarrant`]. The group is raw bytes because
    /// its type lives in a crate this one does not depend on.
    Governance {
        /// The group the op is published on.
        group: [u8; 32],
        /// Which plane of that group.
        kind: GovernanceOpKind,
    },
    /// One new context registered in a group — a
    /// [`crate::ContextCreationWarrant`]. Carries the seed rather than the
    /// context it derives: the derivation (`ContextId::from_seed`) needs a
    /// seeded RNG this crate deliberately does not depend on, so the verifier
    /// that needs the id derives it.
    Creation {
        /// The group the context is registered in.
        group: [u8; 32],
        /// The seed the new context's id is derived from.
        seed: [u8; 32],
    },
}

/// The fields every warrant carries, whatever it authorizes.
///
/// Implemented by the three statement kinds and nothing else: a type that is
/// not signed by an author device over a preimage that commits to all of these
/// is not a warrant, and must not be admitted as one.
pub trait WarrantStatement {
    /// What this warrant authorizes the executor to act on.
    fn scope(&self) -> WarrantScope;
    /// The account the resulting change is authorized for and attributed to.
    fn author_account(&self) -> AccountId;
    /// The device key that signed this warrant.
    fn author_device_key(&self) -> PublicKey;
    /// The operator authorized to act.
    fn executor(&self) -> AccountId;
    /// The one device of [`Self::executor`] that may spend this warrant.
    fn executor_key(&self) -> PublicKey;
    /// Monotonic per author device; what the replay ledger spends.
    fn nonce(&self) -> u64;
    /// Wall-clock bound in seconds — checked by the executor, never at apply.
    fn not_after(&self) -> u64;
    /// The governance heads the author's view descended from.
    fn governance_floor(&self) -> &[[u8; 32]];
    /// Check the signature (and the statement's own bounds) against
    /// [`Self::author_device_key`].
    ///
    /// # Errors
    /// The statement kind's own signature or bound error.
    fn verify_signature(&self) -> Result<(), AccountError>;
}

/// A warrant plus the two certificates that tie its keys to the accounts it
/// names, and the key that signed the change it travels with.
///
/// Self-contained by construction, on the same terms as [`AccountProof`]: a
/// receiver checks the whole bundle without having folded a single prior op
/// about either account, so two replicas with different histories reach the
/// same verdict about who authorized what.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Delegated<W> {
    /// The author's consent.
    ///
    /// Boxed, as the two proofs are: a warrant is a few hundred bytes inline,
    /// and three of those in one enum variant put it far past clippy's
    /// `large_enum_variant` threshold — which matters because these bundles ride
    /// inside the gossip `BroadcastMessage`, the catchup `MessagePayload` and the
    /// governance op enums. Borsh encodes `Box<T>` exactly as `T`, so the boxing
    /// changes no schema. (Borsh's `Box` decoder needs `W: Clone`, hence the
    /// bound; every statement kind is `Clone`.)
    #[borsh(bound(deserialize = "W: BorshDeserialize + Clone"))]
    pub warrant: Box<W>,
    /// Proves the warrant's author device key is a device of its author account.
    ///
    /// This is what lets a device that has never joined the group be an author:
    /// bindings are per group, so a thin client's key is in no group's rows, and
    /// resolving it there would fail. The certificate answers from the account
    /// id alone instead.
    pub author_proof: Box<AccountProof<DeviceCert>>,
    /// Proves [`Self::executor_key`] is a device of the warrant's executor.
    pub executor_proof: Box<AccountProof<DeviceCert>>,
    /// The key that actually signed the change this bundle travelled with.
    ///
    /// Must equal the key the author signed into the warrant, and
    /// `executor_proof` must certify it under the warrant's executor account.
    pub executor_key: PublicKey,
}

impl<W: WarrantStatement + Clone> Delegated<W> {
    /// The author half of [`Self::verify`], which needs nothing from the executor,
    /// so a relay can refuse a forged warrant before doing any work for it.
    ///
    /// # Errors
    /// As [`Self::verify`], for the warrant signature and the author proof.
    pub fn verify_author(
        warrant: &W,
        author_proof: &AccountProof<DeviceCert>,
    ) -> Result<(), AccountError> {
        warrant.verify_signature()?;

        // Each proof gets two steps, and the second is the one that is easy to
        // skip. `verify` establishes that the certificate genuinely came from
        // that account's root — it says nothing about WHICH key the certificate
        // is about. Without the equality below, a perfectly valid certificate for
        // one of the account's other devices would vouch for a key that account
        // never certified.
        let author_cert = author_proof.verify(warrant.author_account())?;
        if author_cert.sign_pk != warrant.author_device_key() {
            return Err(AccountError::WarrantProofKeyMismatch);
        }
        Ok(())
    }

    /// Check the bundle's authenticity: the warrant is signed by the device it
    /// names, and both named keys belong to the accounts the warrant names.
    ///
    /// **Authenticity, not authority.** Revocation, membership, capability,
    /// nonce reuse and expiry all need a cut or a clock and belong to the
    /// caller — the governance store's `warrant_admission` for the standing and
    /// the nonce, the relay's API boundary for the clock.
    ///
    /// # Errors
    /// The statement's own signature error if the warrant is not signed by the
    /// device it names; whatever [`AccountProof::verify`] returns if either
    /// certificate is not genuinely root-signed for the account claimed; and
    /// [`AccountError::WarrantProofKeyMismatch`] if a certificate verifies but
    /// certifies a key other than the one it is supposed to vouch for; and
    /// [`AccountError::WarrantExecutorKeyMismatch`] if the bundle's executor
    /// key is not the one the warrant names.
    pub fn verify(&self) -> Result<Verified<W>, AccountError> {
        Self::verify_author(&self.warrant, &self.author_proof)?;

        let executor_cert = self.executor_proof.verify(self.warrant.executor())?;
        if executor_cert.sign_pk != self.executor_key {
            return Err(AccountError::WarrantProofKeyMismatch);
        }
        if self.executor_key != self.warrant.executor_key() {
            return Err(AccountError::WarrantExecutorKeyMismatch {
                named: self.warrant.executor_key(),
                presented: self.executor_key,
            });
        }

        // Cloned rather than moved: every statement kind owns `Vec`s (and two
        // own `String`s), so none is `Copy`. One clone per verified bundle,
        // against the Ed25519 verifications just done.
        Ok(Verified::new((*self.warrant).clone()))
    }
}
