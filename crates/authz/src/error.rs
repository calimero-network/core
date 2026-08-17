//! Why an op was refused.
//!
//! One enum for every plane, so a caller never has to know which plane said no —
//! and so a new plane cannot quietly introduce a rejection shape the callers
//! don't handle.

use thiserror::Error as ThisError;

use calimero_account::{AccountId, DeviceId};
use calimero_storage::entities::OpMask;

/// Why an op was refused. One rejection type for every plane — the caller
/// doesn't have to know which plane said no.
#[derive(Clone, Debug, PartialEq, Eq, ThisError)]
pub enum Rejected {
    /// Author lacks the required capability on a data entity.
    #[error("author not permitted to write entity (needs {required:?})")]
    NotPermitted { required: OpMask },
    /// Author is not the owner of the object whose writers are being set.
    #[error("author is not the owner of the object")]
    NotOwner,
    /// Author is not an admin of the group being mutated.
    #[error("author is not an admin of the group at the cut")]
    NotGroupAdmin,
    /// Author is not the scope's root admin.
    #[error("author is not the scope root admin at the cut")]
    NotRootAdmin,
    /// The signing device has no binding at this cut, so it speaks for nobody.
    #[error("device {device} is not linked to any account at the cut")]
    DeviceNotLinked {
        /// The unbound device.
        device: DeviceId,
    },
    /// The device is bound, but to a different account than the op claims.
    #[error("device {device} speaks for {bound}, not the claimed {claimed}")]
    DeviceAccountMismatch {
        /// The device that signed.
        device: DeviceId,
        /// The account the device is actually bound to.
        bound: AccountId,
        /// The account the op claimed.
        claimed: AccountId,
    },
    /// A root-key rotation authored by an account other than the one it rotates.
    ///
    /// Its own variant rather than [`Rejected::DeviceAccountMismatch`], which used
    /// to be reused here: that one describes a device bound to a different account
    /// than the op claims, and a rotation involves no device binding at all. Reusing
    /// it produced a message that named the two accounts in the wrong roles — the
    /// author's account is the *established* one (`check_device_speaks_for_author`
    /// has already proved it), and the handoff's account is the claim.
    #[error("rotation of {account} was authored by {author}, which is not that account")]
    RotationNotByAccount {
        /// The account whose root key the handoff would roll.
        account: AccountId,
        /// The account that actually authored the op.
        author: AccountId,
    },
    /// The op was signed with a key the device has since rotated away from.
    #[error("device {device} signed with a superseded key")]
    DeviceKeyStale {
        /// The device whose key is out of date.
        device: DeviceId,
    },
    /// The device's binding has been withdrawn at or before this cut.
    #[error("device {device} was revoked at or before this cut")]
    DeviceRevoked {
        /// The withdrawn device.
        device: DeviceId,
    },
    /// A device certificate did not verify against its self-certifying genesis.
    #[error("device certificate is not internally valid: {reason}")]
    CredentialInvalid {
        /// Why `calimero-account` refused the credential.
        reason: calimero_account::AccountError,
    },
    /// A credential minted by a root key this scope has already superseded.
    #[error("credential signed by superseded key epoch {signed} (current is {current})")]
    CredentialSuperseded {
        /// Epoch the credential was signed under.
        signed: u32,
        /// Epoch currently in force at the cut.
        current: u32,
    },
    /// The link does not advance the device's rotation epoch, so it grants
    /// nothing and would only let an old certificate be replayed.
    #[error("device link at epoch {offered} does not supersede the folded epoch {folded}")]
    DeviceEpochNotAdvanced {
        /// Epoch offered by the incoming link.
        offered: u32,
        /// Epoch already in force.
        folded: u32,
    },
    /// A device may not be moved between accounts; enroll a fresh device id.
    #[error("device is already bound to a different account")]
    DeviceAccountReassignment,
    /// The account is not a member of this scope, so its devices may not link
    /// themselves in.
    #[error("account is not a member of this scope at the cut")]
    AccountNotMember,
    /// A key rotation for an account this scope has never seen, or one that
    /// does not continue the established chain.
    #[error("key rotation does not continue this account's chain at the cut")]
    RotationNotContinuous,
    /// A key rotation not signed by the outgoing root key.
    #[error("key rotation is not signed by the outgoing root key")]
    RotationSignatureInvalid,
}
