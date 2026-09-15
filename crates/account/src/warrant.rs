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
//! | `executor_key` | key | the envelope signature over the change itself |
//!
//! `executor` is an account rather than a key so that a relay re-keying does not
//! void warrants already in flight — a device that re-keys keeps its replica slot
//! by design, and a warrant sitting unspent on an offline client should survive
//! the same rotation. Which *process* signed is [`Delegation::executor_key`],
//! outside the warrant, so the author never has to know it.
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
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::device::DeviceCert;
use crate::domain::{WARRANT_INTENT_DOMAIN, WARRANT_SIGN_DOMAIN};
use crate::error::AccountError;
use crate::signed::{sign_payload, AccountProof, Verified};

/// An author's authorization for one executor to perform one intent, once.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
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
    /// The exact application this warrant authorizes, as the content address of
    /// its bytecode.
    ///
    /// Pins the code, not a version string, so a relay cannot wait for an
    /// upgrade that widens what the named method does and then spend a warrant
    /// signed against the narrower one. A semver would not do: two builds can
    /// share a version and differ in exactly the way that matters.
    pub app_version: ApplicationId,
    /// The method this warrant authorizes, in the clear.
    ///
    /// Carried so a peer can select a **per-method write-set** directly, without
    /// having to reverse a hash against the app's ABI. The arguments are still
    /// committed to rather than carried — see [`Self::intent_hash`].
    pub method: String,
    /// `H(method ‖ args)`. Never the plaintext; see the module header.
    pub intent_hash: [u8; 32],
    /// The account-log heads the author saw when signing.
    ///
    /// Bounded on decode by [`MAX_WARRANT_CITED_HEADS`]: this arrives from
    /// untrusted bytes, and an unbounded set would be an allocation primitive
    /// handed to exactly the party a warrant protects against.
    pub account_heads: Vec<[u8; 32]>,
    /// The governance heads the author's view descended from.
    ///
    /// Bounded on decode by [`MAX_WARRANT_CITED_HEADS`], for the same reason as
    /// [`Self::account_heads`].
    ///
    /// **Its provenance is only as good as its source, and that is an accepted
    /// risk rather than a guarantee.** A floor the relay supplied is a floor the
    /// relay chose: a relay that withholds a governance op can hand the author a
    /// stale view and collect a warrant citing it. Closing that needs the value
    /// to come from the account's own devices or a co-signing peer outside the
    /// relay's operator. Until it does, this detects an honest relay's staleness
    /// and does not constrain a dishonest one.
    pub governance_floor: Vec<[u8; 32]>,
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

/// Most cited heads a warrant may carry, per field.
///
/// Applied before any signature work, for the reason
/// [`MAX_ROOT_KEY_HANDOFFS`](crate::MAX_ROOT_KEY_HANDOFFS) is: a warrant arrives
/// from untrusted bytes, and an unbounded `Vec` there is an allocation primitive
/// handed to exactly the party the warrant protects against.
pub const MAX_WARRANT_CITED_HEADS: usize = 64;

/// The values a warrant is minted from, minus the ones derived for you.
///
/// A struct rather than eleven arguments to [`Warrant::sign`]: four of them
/// are `[u8; 32]` and two are `Vec<[u8; 32]>`, so positionally they are
/// interchangeable to the compiler and not to the verifier. Named fields
/// make a swap a compile error instead of a signature that verifies against
/// the wrong thing.
///
/// `author_device_key` is absent on purpose — it is derived from the secret,
/// so a caller cannot name a key it does not hold.
#[derive(Clone, Debug)]
pub struct WarrantTerms {
    /// The context the intent runs in.
    pub context: ContextId,
    /// The account the change is authorized for and attributed to.
    pub author_account: AccountId,
    /// The operator authorized to act.
    pub executor: AccountId,
    /// The exact application, as the content address of its bytecode.
    pub app_version: ApplicationId,
    /// The method, in the clear.
    pub method: String,
    /// `H(method ‖ args)` — see [`Warrant::intent_hash`].
    pub intent_hash: [u8; 32],
    /// Account-log heads the author saw.
    pub account_heads: Vec<[u8; 32]>,
    /// Governance heads the author's view descended from.
    pub governance_floor: Vec<[u8; 32]>,
    /// Monotonic per author device.
    pub nonce: u64,
    /// Wall-clock bound, in seconds.
    pub not_after: u64,
}

impl Warrant {
    /// The canonical bytes an author signs.
    ///
    /// Takes `&self` rather than a parameter per field: with the v2 field set
    /// that list runs to eleven, and eleven positional arguments of which four
    /// are `[u8; 32]` is a swap waiting to happen. The struct already names them.
    ///
    /// Covers every field except the signature itself.
    #[must_use]
    pub fn signing_payload(&self) -> [u8; 32] {
        // Each cited-head list is preceded by its own length. `domain_hash`
        // length-prefixes every part, so the heads are individually unambiguous
        // -- but the two lists are adjacent, and without the counts
        // `account_heads = [a, b], governance_floor = []` would hash identically
        // to `[a], [b]`.
        let account_len = (self.account_heads.len() as u64).to_le_bytes();
        let governance_len = (self.governance_floor.len() as u64).to_le_bytes();

        let mut parts: Vec<&[u8]> =
            Vec::with_capacity(10 + self.account_heads.len() + self.governance_floor.len());
        parts.push(self.context.digest());
        parts.push(self.author_account.as_bytes());
        parts.push(AsRef::<[u8; 32]>::as_ref(&self.author_device_key));
        parts.push(self.executor.as_bytes());
        parts.push(AsRef::<[u8; 32]>::as_ref(&self.app_version));
        parts.push(self.method.as_bytes());
        parts.push(&self.intent_hash);
        parts.push(&account_len);
        for head in &self.account_heads {
            parts.push(head);
        }
        parts.push(&governance_len);
        for head in &self.governance_floor {
            parts.push(head);
        }
        let nonce = self.nonce.to_le_bytes();
        let not_after = self.not_after.to_le_bytes();
        parts.push(&nonce);
        parts.push(&not_after);

        domain_hash(WARRANT_SIGN_DOMAIN, &parts)
    }

    /// The commitment a warrant carries in place of the intent itself.
    ///
    /// Canonical, and it has to be: the client computes it before signing and
    /// the executor recomputes it from what actually arrived, so a warrant only
    /// authorises the intent it was minted for. Without one function producing
    /// both, the two ends could disagree about what the field covers and the
    /// warrant would authorise whatever the executor chose to run.
    ///
    /// The method is length-prefixed by `domain_hash`, so a method/args split
    /// cannot be shifted across the boundary — `send("ab", x)` and `send("a",
    /// "bx")` commit to different values.
    #[must_use]
    pub fn intent_hash(method: &str, args: &[u8]) -> [u8; 32] {
        domain_hash(WARRANT_INTENT_DOMAIN, &[method.as_bytes(), args])
    }

    /// Whether this warrant was minted for exactly this intent.
    ///
    /// The executor's own check, and the one that stops a warrant being a blank
    /// cheque: everything else establishes that the member signed *something*.
    #[must_use]
    pub fn covers_intent(&self, method: &str, args: &[u8]) -> bool {
        self.intent_hash == Self::intent_hash(method, args)
    }

    /// Mint a warrant, signed by the author's device key.
    ///
    /// `author_device_key` is derived from `author_device_sk` rather than taken as
    /// an argument, for the reason [`crate::PairingOffer::signed`] does the same:
    /// a caller able to name a key it does not hold could produce a warrant it
    /// cannot sign, and the field would stop meaning "who authorized this".
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign, or
    /// [`AccountError::WarrantTooManyCitedHeads`] if either cited-head list is
    /// over [`MAX_WARRANT_CITED_HEADS`] — refused at mint so a warrant that
    /// could never be verified is never produced.
    pub fn sign(author_device_sk: &PrivateKey, terms: WarrantTerms) -> Result<Self, AccountError> {
        let mut warrant = Self {
            context: terms.context,
            author_account: terms.author_account,
            author_device_key: author_device_sk.public_key(),
            executor: terms.executor,
            app_version: terms.app_version,
            method: terms.method,
            intent_hash: terms.intent_hash,
            account_heads: terms.account_heads,
            governance_floor: terms.governance_floor,
            nonce: terms.nonce,
            not_after: terms.not_after,
            // Placeholder: `signing_payload` covers every field but this one, so
            // its value cannot affect what is signed.
            signature: [0u8; 64],
        };
        warrant.check_cited_head_bounds()?;

        let payload = warrant.signing_payload();
        warrant.signature = sign_payload(author_device_sk, &payload)?;
        Ok(warrant)
    }

    /// Refuse cited-head lists larger than [`MAX_WARRANT_CITED_HEADS`].
    ///
    /// Checked before any Ed25519 work, not after: the point is to bound what an
    /// untrusted warrant can make this node allocate and hash, and a check that
    /// runs after the expensive part bounds nothing.
    fn check_cited_head_bounds(&self) -> Result<(), AccountError> {
        for len in [self.account_heads.len(), self.governance_floor.len()] {
            if len > MAX_WARRANT_CITED_HEADS {
                return Err(AccountError::WarrantTooManyCitedHeads {
                    len,
                    max: MAX_WARRANT_CITED_HEADS,
                });
            }
        }
        Ok(())
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
        self.check_cited_head_bounds()?;
        self.author_device_key
            .verify_raw_signature(&self.signing_payload(), &self.signature)
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
    ///
    /// Boxed, as the two proofs are and for the same reason: a warrant is ~264
    /// bytes inline, and three of those in one enum variant put it far past
    /// clippy's `large_enum_variant` threshold — which matters because this type
    /// rides inside the gossip `BroadcastMessage` and the catchup
    /// `MessagePayload`. Borsh encodes `Box<T>` exactly as `T`, so the boxing is
    /// invisible on the wire and changes no schema version.
    pub warrant: Box<Warrant>,
    /// Proves [`Warrant::author_device_key`] is a device of
    /// [`Warrant::author_account`].
    ///
    /// This is what lets a device that has never joined the group be an author:
    /// bindings are per group, so a thin client's key is in no group's rows, and
    /// resolving it there would fail. The certificate answers from the account id
    /// alone instead.
    pub author_proof: Box<AccountProof<DeviceCert>>,
    /// Proves [`Self::executor_key`] is a device of [`Warrant::executor`].
    pub executor_proof: Box<AccountProof<DeviceCert>>,
    /// The key that actually signed the change this bundle travelled with.
    ///
    /// Carried here rather than supplied by the caller, and it is safe despite
    /// looking like a credential nominating its own verifier: the chain closes
    /// it. The warrant names the executor ACCOUNT and is signed by the author,
    /// and `executor_proof` must show this key is a device of that account. So
    /// substituting a key means holding a root-signed certificate for it under
    /// the operator the author actually authorized — which is that operator
    /// acting, not an impersonation of it.
    pub executor_key: PublicKey,
}

impl Delegation {
    /// Check the bundle's authenticity: the warrant is signed by the device it
    /// names, and both named keys belong to the accounts the warrant names.
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
    pub fn verify(&self) -> Result<Verified<Warrant>, AccountError> {
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
        if executor_cert.sign_pk != self.executor_key {
            return Err(AccountError::WarrantProofKeyMismatch);
        }

        // Cloned rather than moved: the v2 field set owns a `String` and two
        // `Vec`s, so `Warrant` is no longer `Copy`. The clone is one per
        // verified delegation, against the Ed25519 verifications just done.
        Ok(Verified::new((*self.warrant).clone()))
    }
}

/// A warrant whose signature and both account bindings have been checked.
///
/// Authenticity only — see [`Delegation::verify`] for what remains the caller's.
pub type VerifiedWarrant = Verified<Warrant>;
