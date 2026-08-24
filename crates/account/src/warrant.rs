//! Delegated authorship: an author's authorization for one executor to perform
//! one intent, and the self-contained bundle that travels with the change it
//! produced.
//!
//! # Why it is shaped this way
//!
//! **A warrant is signed by a device, not by the root.** Every other credential
//! here is root-signed and therefore an [`AccountProof`]; this one is minted per
//! request by the device making it, so the root stays wherever its holder keeps
//! it. That is the whole point — a member authorizing a relay to act for them
//! must not have to reach for the key that mints devices.
//!
//! **It names accounts *and* keys, because those answer different questions.**
//! An account is a content hash, so it is not something a signature verifies
//! against; a key is not what governance rows are keyed by. Every field here is
//! one or the other on purpose:
//!
//! | field | kind | consumed by |
//! | --- | --- | --- |
//! | `author_account` | account | membership, writer sets, the owner stamp |
//! | `author_device_key` | key | this warrant's signature, and the replica slot |
//! | `executor` | account | the authorship capability check |
//!
//! `executor` is an account rather than a key so that a relay re-keying does not
//! void warrants already in flight — a device that re-keys keeps its replica slot
//! by design, and a warrant sitting unspent on an offline client should survive
//! the same rotation. Which *process* actually signed is recorded beside the
//! change it produced, not here.
//!
//! **The intent travels as a hash.** The envelope a delegated change rides in is
//! plaintext to anything subscribed to the topic, members and non-members alike.
//! Naming the method and its arguments here would broadcast application-level
//! intent network-wide, so [`Warrant::intent_hash`] commits to them while the
//! detail stays sealed alongside the operations. A key-holder decrypts and checks
//! the hash matches; everyone else verifies consent without learning what was
//! asked.
//!
//! **This module knows about contexts, and nothing else in the crate does.** A
//! warrant that did not name its scope would authorise the same intent
//! everywhere, which is not a warrant. `ContextId` is a newtype from
//! `calimero-primitives`, already a dependency, so the cost is a concept rather
//! than an edge.
//!
//! # What verification here does not settle
//!
//! [`Delegation::verify`] establishes only what a self-contained credential can:
//! that the warrant was signed by the device it names, and that both named keys
//! genuinely belong to the accounts the warrant names. It is deliberately blind
//! to everything that needs a causal cut or a clock —
//!
//! * has either device been **revoked** in this group?
//! * is `author_account` a **member** at the cut the change cites?
//! * does `executor` hold the **authorship capability** on the owning group?
//! * has this `nonce` already been spent by this author device?
//! * is `not_after` in the past?
//!
//! — because none of them are properties of the bundle. They belong to the
//! projection, to `calimero-authz`, and to the receive path, which are the only
//! places that see a cut or a clock. A caller that checks only what is here has
//! checked authenticity and not authority.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::device::DeviceCert;
use crate::domain::WARRANT_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::signed::{sign_payload, AccountProof, Verified};

/// An author's authorization for one executor to perform one intent, once.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Warrant {
    /// The context the intent runs in. A warrant is scoped, or it authorises the
    /// same request everywhere.
    pub context: ContextId,
    /// The account the resulting change is authorized for and attributed to.
    ///
    /// Carried rather than derived from [`Self::author_device_key`]: re-deriving
    /// it at apply time asks a different question than the author answered, so a
    /// disagreement with the folded binding has to be refusable rather than
    /// silently resolved one way.
    pub author_account: AccountId,
    /// The device key that signed this warrant, and the replica the change is
    /// attributed to.
    pub author_device_key: PublicKey,
    /// The operator authorized to act — an account, so that one of its processes
    /// re-keying does not void warrants already issued to it.
    pub executor: AccountId,
    /// `H(method ‖ args)`. Never the plaintext; see the module header.
    pub intent_hash: [u8; 32],
    /// Monotonic per author **device**.
    ///
    /// Per device rather than per account because two devices of one account are
    /// independent replicas: they cannot coordinate on a shared counter, so an
    /// account-scoped sequence would have them refusing each other's warrants.
    pub nonce: u64,
    /// Wall-clock bound, in seconds. Checked by whoever holds a clock — not here.
    pub not_after: u64,
    /// Signature by [`Self::author_device_key`] over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

impl Warrant {
    /// The canonical bytes an author signs.
    ///
    /// Assembled in one place so the client that mints a warrant, the executor
    /// that spends it and every peer that checks it cannot drift. Covers every
    /// field except the signature itself.
    #[must_use]
    pub fn signing_payload(
        context: ContextId,
        author_account: AccountId,
        author_device_key: &PublicKey,
        executor: AccountId,
        intent_hash: &[u8; 32],
        nonce: u64,
        not_after: u64,
    ) -> [u8; 32] {
        domain_hash(
            WARRANT_SIGN_DOMAIN,
            &[
                context.digest(),
                author_account.as_bytes(),
                AsRef::<[u8; 32]>::as_ref(author_device_key),
                executor.as_bytes(),
                intent_hash,
                &nonce.to_le_bytes(),
                &not_after.to_le_bytes(),
            ],
        )
    }

    /// Mint a warrant, signed by the author's device key.
    ///
    /// `author_device_key` is derived from `author_device_sk` rather than taken as
    /// an argument, for the reason [`crate::PairingOffer::signed`] does the same:
    /// a caller able to name a key it does not hold could produce a warrant it
    /// cannot sign, and the field would stop meaning "who authorized this".
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        author_device_sk: &PrivateKey,
        context: ContextId,
        author_account: AccountId,
        executor: AccountId,
        intent_hash: [u8; 32],
        nonce: u64,
        not_after: u64,
    ) -> Result<Self, AccountError> {
        let author_device_key = author_device_sk.public_key();
        let payload = Self::signing_payload(
            context,
            author_account,
            &author_device_key,
            executor,
            &intent_hash,
            nonce,
            not_after,
        );
        Ok(Self {
            context,
            author_account,
            author_device_key,
            executor,
            intent_hash,
            nonce,
            not_after,
            signature: sign_payload(author_device_sk, &payload)?,
        })
    }

    /// The payload this warrant's own fields address.
    #[must_use]
    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            self.context,
            self.author_account,
            &self.author_device_key,
            self.executor,
            &self.intent_hash,
            self.nonce,
            self.not_after,
        )
    }

    /// Check the signature against the device key the warrant names.
    ///
    /// Establishes that whoever holds `author_device_key` produced this warrant.
    /// It says nothing about whether that key speaks for `author_account` — see
    /// [`Delegation::verify`], which is where the two are tied together.
    ///
    /// # Errors
    /// [`AccountError::WarrantSignatureInvalid`] if the signature does not verify.
    pub fn verify_signature(&self) -> Result<(), AccountError> {
        self.author_device_key
            .verify_raw_signature(&self.payload(), &self.signature)
            .map_err(|_ignored| AccountError::WarrantSignatureInvalid)
    }

    /// Whether this warrant was issued for `context` and to `executor`.
    ///
    /// Separate from [`Self::verify_signature`] because a validly signed warrant
    /// presented in the wrong place is a different failure from a forged one, and
    /// sends whoever reads the error somewhere different.
    ///
    /// # Errors
    /// [`AccountError::WarrantContextMismatch`] or
    /// [`AccountError::WarrantExecutorMismatch`].
    pub fn authorises(&self, context: ContextId, executor: AccountId) -> Result<(), AccountError> {
        if self.context != context {
            return Err(AccountError::WarrantContextMismatch);
        }
        if self.executor != executor {
            return Err(AccountError::WarrantExecutorMismatch {
                named: self.executor,
                expected: executor,
            });
        }
        Ok(())
    }
}

/// What rides alongside a delegated change: the author's consent, plus the two
/// certificates that tie the keys involved to the accounts the warrant names.
///
/// Self-contained by construction, on the same terms as [`AccountProof`]: a
/// receiver checks the whole bundle without having folded a single prior op about
/// either account, so two replicas with different histories reach the same
/// verdict about who authorized what.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Delegation {
    /// The author's consent.
    pub warrant: Warrant,
    /// Proves [`Warrant::author_device_key`] is a device of
    /// [`Warrant::author_account`].
    ///
    /// This is what lets a device that has never joined the group be an author:
    /// bindings are per group, so a thin client's key is in no group's rows, and
    /// resolving it there would fail. The certificate answers from the account id
    /// alone instead.
    pub author_proof: Box<AccountProof<DeviceCert>>,
    /// Proves the key that signed the change is a device of
    /// [`Warrant::executor`].
    pub executor_proof: Box<AccountProof<DeviceCert>>,
}

// Both proofs are boxed for the reason `JoinAccountCredential` is: a credential
// is a few hundred bytes, and inlining two of them pushes any enclosing enum
// variant well past clippy's `large_enum_variant` threshold. Borsh encodes
// `Box<T>` exactly as `T`, so the boxing is invisible on the wire.

impl Delegation {
    /// Check the bundle's authenticity: the warrant is signed by the device it
    /// names, and both named keys belong to the accounts the warrant names.
    ///
    /// `executor_key` is the key that actually signed the change this bundle
    /// travelled with. It is a parameter rather than a field because the bundle
    /// must not be able to nominate its own verifier — the caller passes the key
    /// it verified the change under, and this confirms that key was entitled to
    /// act for the operator the author authorized.
    ///
    /// **What it does not check** is listed in the module header, and the list is
    /// longer than this function: revocation, membership, capability, nonce reuse
    /// and expiry all need a cut or a clock and belong to the caller.
    ///
    /// # Errors
    /// [`AccountError::WarrantSignatureInvalid`] if the warrant is not signed by
    /// the device it names; whatever [`AccountProof::verify`] returns if either
    /// certificate is not genuinely root-signed for the account claimed; and
    /// [`AccountError::WarrantProofKeyMismatch`] if a certificate verifies but
    /// certifies a key other than the one it is supposed to vouch for.
    pub fn verify(&self, executor_key: &PublicKey) -> Result<Verified<Warrant>, AccountError> {
        self.warrant.verify_signature()?;

        // Each proof gets two steps, and the second is the one that is easy to
        // skip. `verify` establishes that the certificate genuinely came from
        // that account's root — it says nothing about WHICH key the certificate
        // is about. Without the equality below, a perfectly valid certificate for
        // one of the account's other devices would vouch for a key that account
        // never certified.
        let author_cert = self.author_proof.verify(self.warrant.author_account)?;
        if author_cert.sign_pk != self.warrant.author_device_key {
            return Err(AccountError::WarrantProofKeyMismatch);
        }

        let executor_cert = self.executor_proof.verify(self.warrant.executor)?;
        if executor_cert.sign_pk != *executor_key {
            return Err(AccountError::WarrantProofKeyMismatch);
        }

        Ok(Verified::new(self.warrant))
    }
}

/// A warrant whose signature and both account bindings have been checked.
///
/// Authenticity only — see [`Delegation::verify`] for what remains the caller's.
pub type VerifiedWarrant = Verified<Warrant>;
