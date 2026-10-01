//! Delegated context creation: an author's authorization for one executor to
//! create one context, in one group, running one application.
//!
//! # Why it is shaped this way
//!
//! **It is not a [`Warrant`](crate::Warrant).** A warrant authorises one intent
//! in a context that already exists, and names that context. Creation has no
//! context to name yet, and what it needs pinned is different: the group the
//! context is registered in, the application it runs, and the arguments `init`
//! runs with. Folding that into a warrant with a reserved method name would make
//! every warrant verifier learn which method names are not methods, and would
//! leave the group — the one thing the author's `CAN_CREATE_CONTEXT` is checked
//! against — as a choice the relay makes. So this is its own statement, under
//! its own signing domain, and neither can be presented as the other.
//!
//! **The context is pinned by its seed.** The context id is derived from a
//! 32-byte seed (the same derivation a seeded `POST /admin-api/contexts` uses),
//! and the author signs the seed rather than the id. That keeps a non-Rust
//! signer from having to reproduce the derivation, and still fixes the id: every
//! peer derives it from the seed and refuses a registration naming any other.
//! One seed, one id, so a warrant can create at most one context — replaying it
//! names a context that is already registered.
//!
//! **Everything that shapes the new context is signed.** Group, seed,
//! application, service, name and the `init` arguments (as a hash) are all in
//! the preimage, so the relay chooses none of them. The executor is named as an
//! account, for the same reason as on a warrant: a relay that re-keys must not
//! void creation warrants already in flight.
//!
//! # What verification here does not settle
//!
//! [`ContextCreationDelegation::verify`] is authenticity only: the statement is
//! signed by the device it names, and both named keys belong to the accounts
//! named. Whether the author may create contexts in the group, whether the
//! executor may act for members there, whether either device is revoked, and
//! whether `not_after` has passed all need a cut or a clock, and belong to the
//! governance apply path and the relay's API boundary respectively.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::delegated::{Delegated, WarrantScope, WarrantStatement};
use crate::domain::{CREATION_INIT_DOMAIN, CREATION_SIGN_DOMAIN};
use crate::error::AccountError;
use crate::signed::{sign_payload, Verified};
use crate::warrant::MAX_WARRANT_CITED_HEADS;

/// Longest `service_name` or `name` a creation warrant may carry, in bytes.
///
/// Both arrive from untrusted bytes and both end up in governance rows every
/// peer stores, so they are bounded here, before any signature work, the way
/// cited heads are.
pub const MAX_CREATION_LABEL_LEN: usize = 256;

/// An author's authorization for one executor to create one context, once.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextCreationWarrant {
    /// The group the context is registered in, as its raw 32-byte id.
    ///
    /// Raw bytes rather than `ContextGroupId`: that type lives in
    /// `calimero-context-config`, which this crate does not depend on.
    pub group: [u8; 32],
    /// The seed the context id is derived from.
    pub seed: [u8; 32],
    /// The account the new context is created for and attributed to.
    pub author_account: AccountId,
    /// The device key that signed this warrant.
    pub author_device_key: PublicKey,
    /// The operator authorized to carry the creation out.
    pub executor: AccountId,
    /// The application the context runs.
    pub application_id: ApplicationId,
    /// Which service of a multi-service bundle to run, `None` for a
    /// single-service application.
    pub service_name: Option<String>,
    /// The context's display name, recorded on its metadata at registration.
    pub name: Option<String>,
    /// `H(init args)`, see [`Self::init_hash`].
    pub init_hash: [u8; 32],
    /// The account-log heads the author saw when signing.
    pub account_heads: Vec<[u8; 32]>,
    /// The governance heads the author's view descended from.
    pub governance_floor: Vec<[u8; 32]>,
    /// Monotonic per author device, like a warrant's.
    pub nonce: u64,
    /// Wall-clock bound, in seconds, checked only by the executor.
    pub not_after: u64,
    /// The author device's signature over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

/// The fields an author chooses when minting a [`ContextCreationWarrant`].
///
/// Everything but the author device key (derived from the signing secret, so it
/// cannot name a key the minter does not hold) and the signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextCreationTerms {
    /// See [`ContextCreationWarrant::group`].
    pub group: [u8; 32],
    /// See [`ContextCreationWarrant::seed`].
    pub seed: [u8; 32],
    /// See [`ContextCreationWarrant::author_account`].
    pub author_account: AccountId,
    /// See [`ContextCreationWarrant::executor`].
    pub executor: AccountId,
    /// See [`ContextCreationWarrant::application_id`].
    pub application_id: ApplicationId,
    /// See [`ContextCreationWarrant::service_name`].
    pub service_name: Option<String>,
    /// See [`ContextCreationWarrant::name`].
    pub name: Option<String>,
    /// See [`ContextCreationWarrant::init_hash`].
    pub init_hash: [u8; 32],
    /// See [`ContextCreationWarrant::account_heads`].
    pub account_heads: Vec<[u8; 32]>,
    /// See [`ContextCreationWarrant::governance_floor`].
    pub governance_floor: Vec<[u8; 32]>,
    /// See [`ContextCreationWarrant::nonce`].
    pub nonce: u64,
    /// See [`ContextCreationWarrant::not_after`].
    pub not_after: u64,
}

impl ContextCreationWarrant {
    /// The canonical bytes an author signs. Covers every field except the
    /// signature itself.
    ///
    /// The two optional labels are encoded as a presence byte followed by the
    /// value, so `None` and `Some("")` sign differently: a relay must not be
    /// able to turn an unnamed context into one named the empty string, or the
    /// reverse.
    #[must_use]
    pub fn signing_payload(&self) -> [u8; 32] {
        let account_len = (self.account_heads.len() as u64).to_le_bytes();
        let governance_len = (self.governance_floor.len() as u64).to_le_bytes();
        let (service_tag, service) = label_parts(self.service_name.as_deref());
        let (name_tag, name) = label_parts(self.name.as_deref());
        let nonce = self.nonce.to_le_bytes();
        let not_after = self.not_after.to_le_bytes();

        let mut parts: Vec<&[u8]> =
            Vec::with_capacity(16 + self.account_heads.len() + self.governance_floor.len());
        parts.push(&self.group);
        parts.push(&self.seed);
        parts.push(self.author_account.as_bytes());
        parts.push(AsRef::<[u8; 32]>::as_ref(&self.author_device_key));
        parts.push(self.executor.as_bytes());
        parts.push(AsRef::<[u8; 32]>::as_ref(&self.application_id));
        parts.push(&service_tag);
        parts.push(service);
        parts.push(&name_tag);
        parts.push(name);
        parts.push(&self.init_hash);
        parts.push(&account_len);
        for head in &self.account_heads {
            parts.push(head);
        }
        parts.push(&governance_len);
        for head in &self.governance_floor {
            parts.push(head);
        }
        parts.push(&nonce);
        parts.push(&not_after);

        domain_hash(CREATION_SIGN_DOMAIN, &parts)
    }

    /// The commitment the warrant carries in place of the `init` arguments.
    ///
    /// One function for both ends, for the reason [`crate::Warrant::intent_hash`]
    /// is: the client computes it before signing and the executor recomputes it
    /// from what arrived, and two implementations could disagree about what it
    /// covers.
    #[must_use]
    pub fn init_hash(init_args: &[u8]) -> [u8; 32] {
        domain_hash(CREATION_INIT_DOMAIN, &[init_args])
    }

    /// Whether this warrant was minted for exactly these `init` arguments.
    #[must_use]
    pub fn covers_init(&self, init_args: &[u8]) -> bool {
        self.init_hash == Self::init_hash(init_args)
    }

    /// Mint a creation warrant, signed by the author's device key.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign, and the
    /// bound errors of [`Self::check_bounds`] for terms no verifier would
    /// accept — refused at mint so such a warrant is never produced.
    pub fn sign(
        author_device_sk: &PrivateKey,
        terms: ContextCreationTerms,
    ) -> Result<Self, AccountError> {
        let mut warrant = Self {
            group: terms.group,
            seed: terms.seed,
            author_account: terms.author_account,
            author_device_key: author_device_sk.public_key(),
            executor: terms.executor,
            application_id: terms.application_id,
            service_name: terms.service_name,
            name: terms.name,
            init_hash: terms.init_hash,
            account_heads: terms.account_heads,
            governance_floor: terms.governance_floor,
            nonce: terms.nonce,
            not_after: terms.not_after,
            // Placeholder: the payload covers every field but this one.
            signature: [0u8; 64],
        };
        warrant.check_bounds()?;

        let payload = warrant.signing_payload();
        warrant.signature = sign_payload(author_device_sk, &payload)?;
        Ok(warrant)
    }

    /// Refuse cited-head lists and labels over their bounds, before any
    /// Ed25519 work, so an untrusted statement cannot make a verifier allocate
    /// and hash without limit.
    fn check_bounds(&self) -> Result<(), AccountError> {
        for len in [self.account_heads.len(), self.governance_floor.len()] {
            if len > MAX_WARRANT_CITED_HEADS {
                return Err(AccountError::WarrantTooManyCitedHeads {
                    len,
                    max: MAX_WARRANT_CITED_HEADS,
                });
            }
        }
        for label in [self.service_name.as_deref(), self.name.as_deref()]
            .into_iter()
            .flatten()
        {
            if label.len() > MAX_CREATION_LABEL_LEN {
                return Err(AccountError::CreationLabelTooLong {
                    len: label.len(),
                    max: MAX_CREATION_LABEL_LEN,
                });
            }
        }
        Ok(())
    }

    /// Check the signature against the device key the warrant names.
    ///
    /// # Errors
    /// [`AccountError::CreationSignatureInvalid`] if it does not verify, or a
    /// bound error.
    pub fn verify_signature(&self) -> Result<(), AccountError> {
        self.check_bounds()?;
        self.author_device_key
            .verify_raw_signature(&self.signing_payload(), &self.signature)
            .map_err(|_ignored| AccountError::CreationSignatureInvalid)
    }

    /// Whether this warrant was issued for `group` and to `executor`.
    ///
    /// # Errors
    /// [`AccountError::CreationGroupMismatch`] or
    /// [`AccountError::WarrantExecutorMismatch`].
    pub fn authorises(&self, group: [u8; 32], executor: AccountId) -> Result<(), AccountError> {
        if self.group != group {
            return Err(AccountError::CreationGroupMismatch);
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

/// A presence tag and the bytes of an optional label.
fn label_parts(label: Option<&str>) -> ([u8; 1], &[u8]) {
    match label {
        Some(value) => ([1], value.as_bytes()),
        None => ([0], &[]),
    }
}

/// What rides inside the governance op that registers a delegated context: the
/// author's consent, plus the two certificates tying the keys involved to the
/// accounts the warrant names. One instance of [`crate::Delegated`], the bundle
/// every warrant kind shares.
pub type ContextCreationDelegation = Delegated<ContextCreationWarrant>;

impl WarrantStatement for ContextCreationWarrant {
    fn scope(&self) -> WarrantScope {
        WarrantScope::Creation {
            group: self.group,
            seed: self.seed,
        }
    }

    fn author_account(&self) -> AccountId {
        self.author_account
    }

    fn author_device_key(&self) -> PublicKey {
        self.author_device_key
    }

    fn executor(&self) -> AccountId {
        self.executor
    }

    fn nonce(&self) -> u64 {
        self.nonce
    }

    fn not_after(&self) -> u64 {
        self.not_after
    }

    fn governance_floor(&self) -> &[[u8; 32]] {
        &self.governance_floor
    }

    fn verify_signature(&self) -> Result<(), AccountError> {
        Self::verify_signature(self)
    }
}

/// A creation warrant whose signature and both account bindings have been
/// checked. Authenticity only.
pub type VerifiedCreationWarrant = Verified<ContextCreationWarrant>;
