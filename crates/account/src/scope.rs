//! What a device of an account may speak for, and the root-signed statement
//! that says so.
//!
//! # Why it is shaped this way
//!
//! **Root-signed, like [`crate::DeviceRevocation`], and for the same reason.**
//! Only the account root decides a device's scope, and "is this signer the
//! account's current root" cannot be answered from folded state without two
//! replicas reaching different verdicts. The statement carries its own proof, so
//! the answer is a property of the statement rather than of the receiver.
//!
//! **Scope is deliberately not a field on [`crate::DeviceCert`].** A certificate
//! travels into every namespace the device is bound in, so carrying the
//! applications on it would tell every member of every project which other
//! projects the account's devices reach. This statement travels only inside the
//! account namespace.
//!
//! **[`DeviceScope::scope_epoch`] orders scopes for one device**, separately from
//! the root-key epoch. The registry keeps a row only when the incoming epoch is
//! above the stored one, so a replayed older statement re-narrows nothing.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey};

use crate::domain::DEVICE_SCOPE_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::signed::{sign_payload, AccountProof, RootSigned, Verified};

/// A root-signed statement of what one device may speak for.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceScope {
    /// The account the device belongs to. Bound into the signature so a scope
    /// cannot be replayed against another account.
    pub account: AccountId,
    /// The device this scope is about.
    pub device: DeviceId,
    /// Applications the device may speak for. **Empty means all of them**, the
    /// same convention a pairing that named none already asks for.
    pub applications: Vec<ApplicationId>,
    /// Orders scopes for this device; only a higher one supersedes.
    pub scope_epoch: u32,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceScope::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceScope {
    /// Canonical bytes the root key signs. Covers every field but the signature.
    ///
    /// The application ids are trailing parts of the same hash rather than a
    /// borsh blob: `domain_hash` length-prefixes each part, so the preimage is
    /// self-delimiting, and encoding here would have to be fallible while
    /// [`RootSigned::payload`] is not.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        device: DeviceId,
        applications: &[ApplicationId],
        scope_epoch: u32,
        key_epoch: u32,
    ) -> [u8; 32] {
        let scope_epoch = scope_epoch.to_le_bytes();
        let key_epoch = key_epoch.to_le_bytes();
        let mut parts: Vec<&[u8]> = Vec::with_capacity(4 + applications.len());
        parts.push(account.as_bytes());
        parts.push(device.as_bytes());
        parts.push(&scope_epoch);
        parts.push(&key_epoch);
        parts.extend(applications.iter().map(|application| &application[..]));
        domain_hash(DEVICE_SCOPE_SIGN_DOMAIN, &parts)
    }

    /// Mint a scope for `device`, signed by the account root at `key_epoch`.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        root_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        applications: Vec<ApplicationId>,
        scope_epoch: u32,
        key_epoch: u32,
    ) -> Result<Self, AccountError> {
        let payload = Self::signing_payload(account, device, &applications, scope_epoch, key_epoch);
        Ok(Self {
            account,
            device,
            applications,
            scope_epoch,
            key_epoch,
            signature: sign_payload(root_sk, &payload)?,
        })
    }
}

impl RootSigned for DeviceScope {
    const ACCOUNT_MISMATCH: AccountError = AccountError::ScopeAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::ScopeSignatureInvalid;

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
            &self.applications,
            self.scope_epoch,
            self.key_epoch,
        )
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

/// A [`DeviceScope`] together with everything needed to verify it.
pub type SignedDeviceScope = AccountProof<DeviceScope>;

/// A [`DeviceScope`] whose anchor, chain, and signature have all been checked.
/// See [`Verified`] for what that does, and does not, mean.
pub type VerifiedDeviceScope = Verified<DeviceScope>;

impl SignedDeviceScope {
    /// Whether this proof states the scope of `device` under `account`.
    ///
    /// Checks the device the caller expects against the one the proof names
    /// before verifying anything, so a valid scope for one device cannot be
    /// presented as another's.
    ///
    /// # Errors
    /// [`AccountError::ScopeDeviceMismatch`] when the proof names a different
    /// device; otherwise whatever [`AccountProof::verify`] reports.
    pub fn authorises(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<VerifiedDeviceScope, AccountError> {
        if self.statement.device != device {
            return Err(AccountError::ScopeDeviceMismatch {
                named: self.statement.device,
                expected: device,
            });
        }
        self.verify(account)
    }
}
