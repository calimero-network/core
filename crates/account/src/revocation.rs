//! Withdrawing a device, and the self-contained proof that authorises it.
//!
//! # Why it is shaped this way
//!
//! **Self-certifying, like [`crate::DeviceCert`], and for the same reason.**
//! Whether the account owner may revoke their own device cannot be answered from
//! folded state: "is the signer this account's current root key" depends on which
//! rotations a given replica has folded, so two replicas would reach different
//! verdicts on one op and disagree permanently about who may author. Carrying the
//! proof makes the answer a property of the op rather than of the receiver.
//!
//! **Any epoch the chain resolves is accepted, not merely the newest.** That is a
//! deliberate asymmetry with a device certificate, whose superseded epochs are
//! filtered when the view is read. Applying the same filter here would mean
//! rotating an account's root key silently *un-revokes* every device it had
//! withdrawn — and a revocation is terminal by design, precisely so a spent
//! [`DeviceId`] can never come back.
//!
//! The cost is that a compromised *old* root key can still revoke devices. That is
//! accepted rather than overlooked: whoever holds any root key of an account can
//! already sign a handoff and take it over, so root-key compromise is
//! unrecoverable regardless, and the terminal-revocation guarantee is worth more
//! than narrowing a capability an attacker in that position already has.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey};

use crate::account::AccountGenesis;
use crate::domain::DEVICE_REVOCATION_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::root_key::RootKeyHandoff;
use crate::signed::{sign_payload, verify_root_signed, AccountProof, RootSigned, Verified};

/// A root-signed withdrawal of a device.
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

    /// Mint a revocation for `device`, signed by the account root at `key_epoch`.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        root_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        key_epoch: u32,
    ) -> Result<Self, AccountError> {
        let payload = Self::signing_payload(account, device, key_epoch);
        Ok(Self {
            account,
            device,
            key_epoch,
            signature: sign_payload(root_sk, &payload)?,
        })
    }
}

impl RootSigned for DeviceRevocation {
    const ACCOUNT_MISMATCH: AccountError = AccountError::RevocationAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::RevocationSignatureInvalid;

    fn account(&self) -> AccountId {
        self.account
    }

    fn key_epoch(&self) -> u32 {
        self.key_epoch
    }

    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(self.account, self.device, self.key_epoch)
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

/// A [`DeviceRevocation`] together with everything needed to verify it.
pub type SignedDeviceRevocation = AccountProof<DeviceRevocation>;

/// A [`DeviceRevocation`] whose anchor, chain, and signature have all been
/// checked. See [`Verified`] for what that does — and does not — mean.
pub type VerifiedDeviceRevocation = Verified<DeviceRevocation>;

impl SignedDeviceRevocation {
    /// Whether this proof authorises withdrawing `device` from `account`.
    ///
    /// Checks the device the caller expects against the one the proof names before
    /// verifying anything, so a valid proof for one device cannot authorise
    /// another.
    ///
    /// # Errors
    /// [`AccountError::RevocationDeviceMismatch`] when the proof names a
    /// different device than the op does; otherwise whatever
    /// [`AccountProof::verify`] reports.
    pub fn authorises(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<VerifiedDeviceRevocation, AccountError> {
        if self.statement.device != device {
            return Err(AccountError::RevocationDeviceMismatch {
                named: self.statement.device,
                expected: device,
            });
        }
        self.verify(account)
    }
}

/// Verify a revocation against the account it names, from the account id alone.
///
/// Takes the anchor and chain separately rather than a
/// [`SignedDeviceRevocation`] so a caller holding a borrowed chain verifies
/// without allocating one; a caller that *has* a proof should call
/// [`SignedDeviceRevocation::authorises`], which also checks the device.
///
/// # Errors
/// See [`verify_root_signed`].
pub fn verify_device_revocation(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    revocation: &DeviceRevocation,
) -> Result<VerifiedDeviceRevocation, AccountError> {
    verify_root_signed(claimed_account, genesis, chain, revocation)?;
    Ok(Verified::new(*revocation))
}
