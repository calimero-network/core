//! What a device of an account is called: a root-signed statement, kept off
//! [`crate::DeviceCert`] so a rename never re-certifies the device.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey};

use crate::domain::DEVICE_LABEL_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::signed::{sign_payload, AccountProof, DeviceBound, RootSigned, Verified};

/// A root-signed name for one device. Display only: it gates nothing.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceLabel {
    /// The account the device belongs to. Bound into the signature so a label
    /// cannot be replayed against another account.
    pub account: AccountId,
    /// The device this label is about.
    pub device: DeviceId,
    /// What to call it.
    pub label: String,
    /// Orders labels for this device; only a higher one supersedes.
    pub label_epoch: u32,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceLabel::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceLabel {
    /// Canonical bytes the root key signs. Covers every field but the signature.
    ///
    /// The label is a trailing hash part like a scope's application ids:
    /// `domain_hash` length-prefixes each part, so a name containing the
    /// separator bytes of another field cannot be shifted into it.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        device: DeviceId,
        label: &str,
        label_epoch: u32,
        key_epoch: u32,
    ) -> [u8; 32] {
        let label_epoch = label_epoch.to_le_bytes();
        let key_epoch = key_epoch.to_le_bytes();
        domain_hash(
            DEVICE_LABEL_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                device.as_bytes(),
                &label_epoch,
                &key_epoch,
                label.as_bytes(),
            ],
        )
    }

    /// Mint a label for `device`, signed by the account root at `key_epoch`.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        root_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        label: String,
        label_epoch: u32,
        key_epoch: u32,
    ) -> Result<Self, AccountError> {
        let payload = Self::signing_payload(account, device, &label, label_epoch, key_epoch);
        Ok(Self {
            account,
            device,
            label,
            label_epoch,
            key_epoch,
            signature: sign_payload(root_sk, &payload)?,
        })
    }
}

impl RootSigned for DeviceLabel {
    const ACCOUNT_MISMATCH: AccountError = AccountError::LabelAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::LabelSignatureInvalid;

    fn account(&self) -> AccountId {
        self.account
    }

    fn key_epoch(&self) -> u32 {
        self.key_epoch
    }

    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            self.account,
            self.device,
            &self.label,
            self.label_epoch,
            self.key_epoch,
        )
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

impl DeviceBound for DeviceLabel {
    const DEVICE_MISMATCH: fn(DeviceId, DeviceId) -> AccountError =
        |named, expected| AccountError::LabelDeviceMismatch { named, expected };

    fn device(&self) -> DeviceId {
        self.device
    }
}

/// A [`DeviceLabel`] together with everything needed to verify it.
pub type SignedDeviceLabel = AccountProof<DeviceLabel>;

/// A [`DeviceLabel`] whose anchor, chain, and signature have all been checked.
/// See [`Verified`] for what that does, and does not, mean.
pub type VerifiedDeviceLabel = Verified<DeviceLabel>;
