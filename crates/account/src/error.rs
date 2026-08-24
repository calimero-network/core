//! Why a credential failed verification.

use thiserror::Error as ThisError;

use calimero_primitives::identity::{AccountId, DeviceId};

/// Why a credential failed verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ThisError)]
pub enum AccountError {
    /// The genesis does not hash to the account id it claims.
    #[error("genesis hashes to {actual}, not the claimed account {claimed}")]
    GenesisMismatch {
        /// The account id the credential claims.
        claimed: AccountId,
        /// The account id the supplied genesis actually addresses.
        actual: AccountId,
    },
    /// The genesis carries a version this build does not understand.
    #[error("unsupported account genesis version {found} (this build supports {supported})")]
    UnsupportedVersion {
        /// Version read from the genesis.
        found: u8,
        /// Version this build mints and accepts.
        supported: u8,
    },
    /// A member endorsement's signature does not verify under the key it names.
    #[error("account endorsement is not validly signed by the member it names")]
    EndorsementSignatureInvalid,
    /// A pairing statement does not verify under the signing key it offers, so
    /// the party presenting this key material is not the party that generated it.
    #[error("pairing statement is not validly signed by the device key it offers")]
    PairingStatementInvalid,
    /// The supplied chain is longer than [`crate::MAX_ROOT_KEY_HANDOFFS`].
    #[error("handoff chain has {found} entries, over the {limit} cap")]
    ChainTooLong {
        /// Length of the supplied chain.
        found: usize,
        /// The cap.
        limit: usize,
    },
    /// A handoff in the chain is not the immediate successor of the previous
    /// one. The chain must start at epoch 0 and step by exactly one.
    #[error("handoff chain not contiguous: expected from_epoch {expected}, found {found}")]
    ChainNotContiguous {
        /// The epoch the chain position requires.
        expected: u32,
        /// The epoch the handoff actually declares.
        found: u32,
    },
    /// A handoff names a different account than the one being verified.
    #[error("handoff at epoch {epoch} is for a different account")]
    HandoffAccountMismatch {
        /// Chain position of the offending handoff.
        epoch: u32,
    },
    /// A handoff is not validly signed by the outgoing root key.
    #[error("handoff at epoch {epoch} has an invalid signature")]
    HandoffSignatureInvalid {
        /// Chain position of the offending handoff.
        epoch: u32,
    },
    /// The certificate claims a root-key epoch the supplied chain does not
    /// reach.
    #[error("certificate claims key epoch {key_epoch} but the chain only reaches {reachable}")]
    EpochOutOfRange {
        /// Epoch the certificate claims.
        key_epoch: u32,
        /// Highest epoch the supplied chain establishes.
        reachable: u32,
    },
    /// The certificate names a different account than the genesis.
    #[error("certificate is for a different account than the supplied genesis")]
    CertAccountMismatch,
    /// The certificate is not validly signed by the root key at its claimed
    /// epoch.
    #[error("certificate has an invalid signature for its claimed key epoch")]
    CertSignatureInvalid,
    /// The signing key refused to produce a signature.
    #[error("signing failed")]
    SigningFailed,
    /// The revocation names a different account than the genesis.
    #[error("revocation is for a different account than the supplied genesis")]
    RevocationAccountMismatch,
    /// The revocation is not validly signed by the root key at its claimed
    /// epoch.
    #[error("revocation has an invalid signature for its claimed key epoch")]
    RevocationSignatureInvalid,
    /// The revocation names a different device than the caller is withdrawing.
    ///
    /// Distinct from [`Self::RevocationAccountMismatch`] because it sends whoever
    /// reads it somewhere different: the account matched and the *device* did not,
    /// so the proof is a valid one presented against the wrong subject rather
    /// than a proof for the wrong account.
    #[error("revocation is for device {named}, not the {expected} being withdrawn")]
    RevocationDeviceMismatch {
        /// The device the proof actually names.
        named: DeviceId,
        /// The device the caller is withdrawing.
        expected: DeviceId,
    },
    /// The warrant is not validly signed by the device key it names.
    #[error("warrant has an invalid signature for the device key it names")]
    WarrantSignatureInvalid,
    /// The warrant was issued for a different context than the one it is being
    /// presented in.
    #[error("warrant is for a different context than the one it is presented in")]
    WarrantContextMismatch,
    /// The warrant authorises a different operator than the one presenting it.
    #[error("warrant authorises {named}, not the {expected} presenting it")]
    WarrantExecutorMismatch {
        /// The operator the warrant actually authorises.
        named: AccountId,
        /// The operator presenting it.
        expected: AccountId,
    },
    /// A certificate in a delegation verified against its account but certifies a
    /// different key than the one it is supposed to vouch for.
    ///
    /// Its own variant because it is the failure that looks like success:
    /// the proof is genuine and the account is right, so a check that stopped at
    /// [`AccountProof::verify`](crate::AccountProof::verify) would accept it.
    #[error("certificate verifies for its account but certifies a different key")]
    WarrantProofKeyMismatch,
}
