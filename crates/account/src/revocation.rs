//! Withdrawing a device, and the self-contained proof that authorises it.
//!
//! The counterpart of [`crate::DeviceCert`], with one deliberate asymmetry:
//! a revocation stays valid under **any** epoch its chain resolves, because a
//! revocation is terminal — see [`verify_device_revocation`].

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey};

use crate::account::AccountGenesis;
use crate::domain::DEVICE_REVOCATION_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::root_key::{root_key_at_epoch, RootKeyHandoff};

/// A root-signed withdrawal of a device.
///
/// The counterpart of [`crate::DeviceCert`], and self-certifying for the same
/// reason: whether the account owner may revoke their own device cannot be
/// answered from folded state. "Is the signer this account's current root key"
/// depends on which rotations a given replica has folded, so two replicas would
/// reach different verdicts on one op and disagree permanently about who may
/// author. Carrying the proof makes the answer a property of the op rather than
/// of the receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceRevocation {
    /// The account withdrawing the device. Bound into the signature so a
    /// revocation cannot be replayed against another account.
    pub account: AccountId,
    /// The device being withdrawn.
    pub device: DeviceId,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceRevocation::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceRevocation {
    /// Canonical bytes the root key signs. Covers every field but the signature.
    #[must_use]
    pub fn signing_payload(account: AccountId, device: DeviceId, key_epoch: u32) -> [u8; 32] {
        domain_hash(
            DEVICE_REVOCATION_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                device.as_bytes(),
                &key_epoch.to_le_bytes(),
            ],
        )
    }

    /// The bytes this revocation's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(self.account, self.device, self.key_epoch)
    }
}

/// Mint a revocation for `device`, signed by the account root at `key_epoch`.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub fn sign_device_revocation(
    root_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    key_epoch: u32,
) -> Result<DeviceRevocation, AccountError> {
    let payload = DeviceRevocation::signing_payload(account, device, key_epoch);
    Ok(DeviceRevocation {
        account,
        device,
        key_epoch,
        signature: root_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// A [`DeviceRevocation`] together with everything needed to verify it.
///
/// Bundled rather than passed as three fields on the op for the same reason a
/// link carries its genesis and chain: the proof has to stand on its own, so a
/// receiver can check it without having folded any prior op about the account.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SignedDeviceRevocation {
    /// The account's genesis, which hashes to the account being withdrawn from.
    pub genesis: AccountGenesis,
    /// Root-key handoffs up to `revocation.key_epoch`.
    pub chain: Vec<RootKeyHandoff>,
    /// The revocation itself.
    pub revocation: DeviceRevocation,
}

impl SignedDeviceRevocation {
    /// Whether this proof authorises withdrawing `device` from `account`.
    ///
    /// Checks the account and device the caller expects against the ones the
    /// proof names, so a valid proof for one device cannot authorise another.
    ///
    /// # Errors
    /// Propagates [`verify_device_revocation`]; also
    /// [`AccountError::RevocationAccountMismatch`] when the proof is for a
    /// different device than the op names.
    pub fn authorises(&self, account: AccountId, device: DeviceId) -> Result<(), AccountError> {
        if self.revocation.device != device {
            return Err(AccountError::RevocationAccountMismatch);
        }
        verify_device_revocation(account, &self.genesis, &self.chain, &self.revocation)
    }
}

/// Verify a revocation against the account it names, from the account id alone.
///
/// **Any epoch the carried chain resolves is accepted, not merely the newest.**
/// That is a deliberate asymmetry with [`crate::verify_device_cert`], whose
/// superseded epochs are filtered when the view is read. Applying the same filter
/// here would mean rotating an account's root key silently *un-revokes* every
/// device it had withdrawn — and a revocation is terminal by design, precisely so
/// a spent `DeviceId` can never come back.
///
/// The cost is that a compromised *old* root key can still revoke devices. That
/// is accepted rather than overlooked: whoever holds any root key of an account
/// can already sign a handoff and take it over, so root-key compromise is
/// unrecoverable regardless, and the terminal-revocation guarantee is worth more
/// than narrowing a capability an attacker in that position already has.
///
/// # Errors
/// - [`AccountError::GenesisMismatch`] if the genesis does not hash to
///   `claimed_account`.
/// - [`AccountError::RevocationAccountMismatch`] if the revocation names a
///   different account.
/// - [`AccountError::EpochOutOfRange`] if `key_epoch` exceeds the chain.
/// - [`AccountError::RevocationSignatureInvalid`] if the signature does not
///   verify under the key at that epoch.
pub fn verify_device_revocation(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    revocation: &DeviceRevocation,
) -> Result<(), AccountError> {
    let derived = genesis.account_id();
    if derived != claimed_account {
        return Err(AccountError::GenesisMismatch {
            claimed: claimed_account,
            actual: derived,
        });
    }
    if revocation.account != derived {
        return Err(AccountError::RevocationAccountMismatch);
    }

    let signer = root_key_at_epoch(genesis, chain, revocation.key_epoch)?;

    if signer
        .verify_raw_signature(&revocation.payload(), &revocation.signature)
        .is_err()
    {
        return Err(AccountError::RevocationSignatureInvalid);
    }
    Ok(())
}
