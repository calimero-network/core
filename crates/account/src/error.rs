//! Why a credential failed verification.

use thiserror::Error as ThisError;

use calimero_primitives::identity::AccountId;

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
}
