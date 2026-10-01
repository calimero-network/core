//! Delegated governance: an author's authorization for one executor to publish
//! one governance op — add a member, create a subgroup, rename a context — in
//! the author's name.
//!
//! # Why it is shaped this way
//!
//! **It commits to the op, not to a description of it.** The warrant carries
//! `H(kind ‖ op bytes)` over the op's *delegable form* — the op exactly as the
//! member means it, with only the fields a relay has to compute from its own
//! view (a removal's post-state hashes, a cascade's enumerated subtree) cleared.
//! What the op does, to whom, in which group, is therefore signed byte for
//! byte, and the relay chooses nothing about it. This crate treats the bytes as
//! opaque; `calimero-governance-types` owns the op types and the normal form.
//!
//! **`kind` separates the planes.** A group op and a namespace (root) op are
//! different types whose encodings could coincide; the kind byte is inside the
//! commitment so consent to one can never be presented as the other.
//!
//! **`scope` is the group the op is published on** — the group itself for a
//! group op, the namespace for a root op — so a warrant cannot be spent in a
//! group it was not signed for.
//!
//! Everything else — revocation, membership, the author's authority for this
//! particular op, the executor's standing, the nonce, expiry — needs a cut or a
//! clock and is the governance apply path's, exactly as for
//! [`crate::Warrant`] and [`crate::ContextCreationWarrant`].

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::delegated::{Delegated, WarrantScope, WarrantStatement};
use crate::domain::{GOVERNANCE_OP_DOMAIN, GOVERNANCE_SIGN_DOMAIN};
use crate::error::AccountError;
use crate::signed::{sign_payload, Verified};
use crate::warrant::MAX_WARRANT_CITED_HEADS;

/// Which plane a delegated governance op is published on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum GovernanceOpKind {
    /// A `GroupOp`, published on the group's own log.
    Group = 0,
    /// A namespace `RootOp`, published on the namespace log.
    Root = 1,
}

/// An author's authorization for one executor to publish one governance op, once.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct GovernanceWarrant {
    /// The group the op is published on (the namespace, for a root op), raw.
    pub scope: [u8; 32],
    /// Which plane the op is on.
    pub kind: GovernanceOpKind,
    /// The account the op is authorized as and attributed to.
    pub author_account: AccountId,
    /// The device key that signed this warrant.
    pub author_device_key: PublicKey,
    /// The operator authorized to publish it.
    pub executor: AccountId,
    /// `H(kind ‖ delegable op bytes)`, see [`Self::op_hash`].
    pub op_hash: [u8; 32],
    /// The account-log heads the author saw when signing.
    pub account_heads: Vec<[u8; 32]>,
    /// The governance heads the author's view descended from.
    pub governance_floor: Vec<[u8; 32]>,
    /// Monotonic per author device.
    pub nonce: u64,
    /// Wall-clock bound, in seconds, checked only by the executor.
    pub not_after: u64,
    /// The author device's signature over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

/// The fields an author chooses when minting a [`GovernanceWarrant`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernanceTerms {
    /// See [`GovernanceWarrant::scope`].
    pub scope: [u8; 32],
    /// See [`GovernanceWarrant::kind`].
    pub kind: GovernanceOpKind,
    /// See [`GovernanceWarrant::author_account`].
    pub author_account: AccountId,
    /// See [`GovernanceWarrant::executor`].
    pub executor: AccountId,
    /// See [`GovernanceWarrant::op_hash`].
    pub op_hash: [u8; 32],
    /// See [`GovernanceWarrant::account_heads`].
    pub account_heads: Vec<[u8; 32]>,
    /// See [`GovernanceWarrant::governance_floor`].
    pub governance_floor: Vec<[u8; 32]>,
    /// See [`GovernanceWarrant::nonce`].
    pub nonce: u64,
    /// See [`GovernanceWarrant::not_after`].
    pub not_after: u64,
}

impl GovernanceWarrant {
    /// The canonical bytes an author signs. Covers every field except the
    /// signature.
    #[must_use]
    pub fn signing_payload(&self) -> [u8; 32] {
        let kind = [self.kind as u8];
        let account_len = (self.account_heads.len() as u64).to_le_bytes();
        let governance_len = (self.governance_floor.len() as u64).to_le_bytes();
        let nonce = self.nonce.to_le_bytes();
        let not_after = self.not_after.to_le_bytes();

        let mut parts: Vec<&[u8]> =
            Vec::with_capacity(12 + self.account_heads.len() + self.governance_floor.len());
        parts.push(&self.scope);
        parts.push(&kind);
        parts.push(self.author_account.as_bytes());
        parts.push(AsRef::<[u8; 32]>::as_ref(&self.author_device_key));
        parts.push(self.executor.as_bytes());
        parts.push(&self.op_hash);
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

        domain_hash(GOVERNANCE_SIGN_DOMAIN, &parts)
    }

    /// The commitment to an op: `H(kind ‖ op bytes)` under its own domain.
    ///
    /// `op_bytes` is the borsh encoding of the op's delegable form; the one
    /// function both ends call, so they cannot disagree about what it covers.
    #[must_use]
    pub fn op_hash(kind: GovernanceOpKind, op_bytes: &[u8]) -> [u8; 32] {
        domain_hash(GOVERNANCE_OP_DOMAIN, &[&[kind as u8], op_bytes])
    }

    /// Whether this warrant was minted for exactly this op.
    #[must_use]
    pub fn covers_op(&self, kind: GovernanceOpKind, op_bytes: &[u8]) -> bool {
        self.kind == kind && self.op_hash == Self::op_hash(kind, op_bytes)
    }

    /// Mint a governance warrant, signed by the author's device key.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`], or
    /// [`AccountError::WarrantTooManyCitedHeads`] for terms no verifier accepts.
    pub fn sign(
        author_device_sk: &PrivateKey,
        terms: GovernanceTerms,
    ) -> Result<Self, AccountError> {
        let mut warrant = Self {
            scope: terms.scope,
            kind: terms.kind,
            author_account: terms.author_account,
            author_device_key: author_device_sk.public_key(),
            executor: terms.executor,
            op_hash: terms.op_hash,
            account_heads: terms.account_heads,
            governance_floor: terms.governance_floor,
            nonce: terms.nonce,
            not_after: terms.not_after,
            signature: [0u8; 64],
        };
        warrant.check_bounds()?;
        let payload = warrant.signing_payload();
        warrant.signature = sign_payload(author_device_sk, &payload)?;
        Ok(warrant)
    }

    fn check_bounds(&self) -> Result<(), AccountError> {
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
    /// # Errors
    /// [`AccountError::GovernanceSignatureInvalid`], or a bound error.
    pub fn verify_signature(&self) -> Result<(), AccountError> {
        self.check_bounds()?;
        self.author_device_key
            .verify_raw_signature(&self.signing_payload(), &self.signature)
            .map_err(|_ignored| AccountError::GovernanceSignatureInvalid)
    }
}

/// What rides inside a delegated governance op: the author's consent and the
/// two certificates tying the keys to the accounts named. One instance of
/// [`crate::Delegated`], the bundle every warrant kind shares.
pub type GovernanceDelegation = Delegated<GovernanceWarrant>;

impl WarrantStatement for GovernanceWarrant {
    fn scope(&self) -> WarrantScope {
        WarrantScope::Governance {
            group: self.scope,
            kind: self.kind,
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

/// A governance warrant whose signature and both bindings have been checked.
pub type VerifiedGovernanceWarrant = Verified<GovernanceWarrant>;
