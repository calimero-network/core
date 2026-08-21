//! The shape every root-signed credential shares: the trait that names it, the
//! wrapper that records a credential was checked, and the bundle that carries
//! one across the wire.
//!
//! # Why it is shaped this way
//!
//! **One verifier, not one per statement.** A device certificate and a device
//! revocation are checked by the same four steps in the same order — the genesis
//! addresses the account the caller already trusts, the statement names that same
//! account, the handoff chain reaches the epoch the statement claims, and the key
//! at that epoch signed it. Written out twice, those four steps are two things
//! that can drift, and a drift here is not a bug in one credential kind: it is one
//! kind quietly accepting what the other refuses. [`RootSigned`] exists so the
//! steps are written once and each statement kind supplies only what genuinely
//! differs — its fields, and which two [`AccountError`] variants it reports.
//!
//! Factoring the same body into a plain helper does not work: the parameters
//! would be account, epoch, payload, signature and two error variants, six
//! arguments whose only relationship is that one statement supplies all of them.
//! That relationship *is* the trait.
//!
//! **`Verified<T>` is a separate type, not a boolean.** A verifier that returns
//! `Result<(), _>` leaves no trace that anyone called it, so "did we check this"
//! becomes a question about control flow that a reader has to answer by reading
//! backwards. `Verified<T>` cannot be built outside this crate, so holding one is
//! the proof. What it deliberately does **not** mean is that the credential is in
//! force — see the type's own docs.
//!
//! **`AccountProof<T>` is one type because it was already one concept.** An
//! anchor, the chain that reaches the signing epoch, and the statement that epoch
//! signed are never meaningful apart: a caller holding two of the three can decide
//! nothing. Naming the triple is what lets a verifier take one argument instead of
//! three, and what stops the same struct being defined once per statement kind in
//! whichever crate happened to need it first.

use core::ops::Deref;

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{AccountId, PrivateKey};

use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::root_key::{root_key_at_epoch, RootKeyHandoff};

/// Sign `payload` and return the raw 64 signature bytes.
///
/// Every minter in this crate ends in exactly this, and the reason it is one
/// function is the same reason `signing_payload` is: a signing tail written out
/// per credential is a place where one of them can start reporting a different
/// error, or stop reporting one at all.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub(crate) fn sign_payload(
    signer: &PrivateKey,
    payload: &[u8; 32],
) -> Result<[u8; 64], AccountError> {
    Ok(signer
        .sign(payload)
        .map_err(|_| AccountError::SigningFailed)?
        .to_bytes())
}

/// A statement an account's **root** key signs, naming the epoch that signed it.
///
/// Implemented by [`crate::DeviceCert`] and [`crate::DeviceRevocation`].
/// Deliberately **not** implemented by [`crate::AccountMemberEndorsement`], which
/// is signed by a granted *member* key rather than by the account root — that is
/// the whole reason the endorsement exists, and leaving it outside this trait is
/// what makes the difference visible to the compiler rather than only to a reader
/// of prose.
pub trait RootSigned {
    /// Reported when the statement names a different account than the genesis.
    const ACCOUNT_MISMATCH: AccountError;
    /// Reported when the signature does not verify under the resolved root key.
    const SIGNATURE_INVALID: AccountError;

    /// The account this statement is about.
    fn account(&self) -> AccountId;
    /// Which root-key epoch signed it.
    fn key_epoch(&self) -> u32;
    /// The canonical bytes the signature covers.
    fn payload(&self) -> [u8; 32];
    /// The signature itself.
    fn signature(&self) -> &[u8; 64];
}

/// A statement whose anchor, key chain, and signature have all been checked.
///
/// Holding one means the credential is **internally** valid. It does **not** mean
/// the statement is currently in force: whether the signing epoch has since been
/// superseded, whether the device was revoked, and whether the account is even a
/// member of the scope are all at-cut questions that only the projection can
/// answer. This type exists so those two stages cannot be mistaken for one
/// another.
///
/// Derefs to the statement, so the fields read exactly as they do on the
/// unchecked value. The inner value is private, which is what makes the wrapper
/// unforgeable from outside this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verified<T>(T);

impl<T> Verified<T> {
    /// Wrap a statement that has just passed [`verify_root_signed`].
    ///
    /// Private to the crate on purpose: every construction site must be one that
    /// just did the checking.
    pub(crate) const fn new(statement: T) -> Self {
        Self(statement)
    }

    /// The checked statement.
    #[must_use]
    pub const fn get(&self) -> &T {
        &self.0
    }

    /// Unwrap to the statement, discarding the proof that it was checked.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Verified<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// A credential that stands on its own: the account anchor, the handoff chain
/// that reaches the signing epoch, and the statement that epoch signed.
///
/// Self-contained by design, which is what costs the bytes on the wire and what
/// buys the property they pay for: a receiver can check the credential without
/// having folded a single prior op about the account, so two replicas that have
/// seen different histories still reach the same verdict.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountProof<T> {
    /// The account's self-certifying root, which hashes to its [`AccountId`].
    pub genesis: AccountGenesis,
    /// Signed root-key rollovers, epoch 0 upward. Empty when the statement was
    /// signed by the genesis key itself.
    pub chain: Vec<RootKeyHandoff>,
    /// The root-signed statement this proof carries.
    pub statement: T,
}

impl<T: RootSigned + Copy> AccountProof<T> {
    /// Check this proof against the account the caller already trusts.
    ///
    /// `claimed_account` is what ties the credential to something outside it —
    /// typically `op.author`. Without it a caller would happily verify a
    /// perfectly well-formed credential for an account nobody asked about.
    ///
    /// # Errors
    /// See [`verify_root_signed`].
    pub fn verify(&self, claimed_account: AccountId) -> Result<Verified<T>, AccountError> {
        verify_root_signed(claimed_account, &self.genesis, &self.chain, &self.statement)?;
        Ok(Verified::new(self.statement))
    }
}

/// Verify any [`RootSigned`] statement against a self-certifying genesis.
///
/// Checks, in order: the genesis addresses `claimed_account`; the statement is
/// for that same account; the handoff chain is valid and reaches the statement's
/// epoch; and the statement is signed by the root key at that epoch.
///
/// Takes the three pieces separately rather than an [`AccountProof`] so a caller
/// holding a borrowed chain it does not own — the apply paths, which read theirs
/// out of a slice — verifies without allocating one to throw away.
///
/// # Errors
/// [`AccountError::GenesisMismatch`] if the genesis does not address
/// `claimed_account`; `T::ACCOUNT_MISMATCH` if the statement names another
/// account; whatever [`root_key_at_epoch`] reports for an unusable chain; and
/// `T::SIGNATURE_INVALID` if the signature does not verify under the key at the
/// claimed epoch.
pub(crate) fn verify_root_signed<T: RootSigned>(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    statement: &T,
) -> Result<(), AccountError> {
    let derived = genesis.account_id();
    if derived != claimed_account {
        return Err(AccountError::GenesisMismatch {
            claimed: claimed_account,
            actual: derived,
        });
    }
    if statement.account() != derived {
        return Err(T::ACCOUNT_MISMATCH);
    }

    let signer = root_key_at_epoch(genesis, chain, statement.key_epoch())?;

    if signer
        .verify_raw_signature(&statement.payload(), statement.signature())
        .is_err()
    {
        return Err(T::SIGNATURE_INVALID);
    }
    Ok(())
}
