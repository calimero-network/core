//! Device credentials: the two keys a device holds, and the root-signed
//! [`DeviceCert`] that binds them to an account.
//!
//! Verification here is *internal* validity only — see [`Verified`] for what that
//! deliberately does not mean.
//!
//! # Why it is shaped this way
//!
//! **Two keys per device, not one.** A device carries an Ed25519 key that signs
//! and an X25519 [`KemPublicKey`] that receives wrapped scope keys, rather than
//! reusing one key for both. Single-key dual-use across a signature scheme and a
//! Diffie-Hellman is a well-known footgun with no compensating benefit, and the
//! type split makes it impossible to pass one where the other belongs. This crate
//! only carries the bytes; the wrapping itself lives with scope-key delivery.
//!
//! **No expiry field.** Expiry requires participants to agree on wall-clock time,
//! which a causally-ordered system does not provide; a certificate that expires
//! "at" some timestamp would be valid on one node and invalid on another, and
//! authorization would stop converging. Withdrawal is expressed as a revocation
//! op instead — see [`crate::revocation`] — which is causally ordered like every
//! other decision.
//!
//! **The minter takes one parameter per signed field rather than a builder.** The
//! argument list *is* the list of fields the signature covers, and hiding it
//! behind optional setters would make an unsigned field easy to miss.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey, PublicKey};

use crate::account::AccountGenesis;
use crate::domain::DEVICE_CERT_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::root_key::RootKeyHandoff;
use crate::signed::{sign_payload, verify_root_signed, RootSigned, Verified};

/// An X25519 public key used only as a scope-key delivery recipient.
///
/// Deliberately a distinct type from [`PublicKey`] (Ed25519) — see the module
/// docs.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct KemPublicKey([u8; 32]);

impl KemPublicKey {
    /// The raw 32 bytes of this key.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for KemPublicKey {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl core::fmt::Display for KemPublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// A root-signed grant binding a device to an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceCert {
    /// The account this device speaks for.
    pub account: AccountId,
    /// The device being granted.
    pub device: DeviceId,
    /// Ed25519 key the device signs ops with.
    pub sign_pk: PublicKey,
    /// X25519 key wrapped scope keys are delivered to.
    pub kem_pk: KemPublicKey,
    /// Which account root-key epoch signed this certificate.
    pub key_epoch: u32,
    /// Monotone per device. A higher value supersedes lower ones and expresses
    /// **device key rotation** — the same device with fresh keys, keeping its
    /// replica identity — as distinct from enrolling a new device.
    pub device_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceCert::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceCert {
    /// Canonical bytes the root key signs. Covers every field except the
    /// signature itself.
    ///
    /// Both keys are covered, so neither the signing key nor the delivery key
    /// can be substituted in a certificate that otherwise verifies.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        device: DeviceId,
        sign_pk: &PublicKey,
        kem_pk: &KemPublicKey,
        key_epoch: u32,
        device_epoch: u32,
    ) -> [u8; 32] {
        domain_hash(
            DEVICE_CERT_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                device.as_bytes(),
                sign_pk.as_ref(),
                kem_pk.as_bytes(),
                &key_epoch.to_le_bytes(),
                &device_epoch.to_le_bytes(),
            ],
        )
    }

    /// Mint a certificate, signed by the account root key at `key_epoch`.
    ///
    /// `device_epoch` must strictly exceed any epoch already folded for this
    /// device — the projection refuses a link that does not advance it, so a
    /// re-issued certificate that reuses an epoch is inert rather than a
    /// rollback.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        root_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        sign_pk: &PublicKey,
        kem_pk: &KemPublicKey,
        key_epoch: u32,
        device_epoch: u32,
    ) -> Result<Self, AccountError> {
        let payload =
            Self::signing_payload(account, device, sign_pk, kem_pk, key_epoch, device_epoch);
        Ok(Self {
            account,
            device,
            sign_pk: *sign_pk,
            kem_pk: *kem_pk,
            key_epoch,
            device_epoch,
            signature: sign_payload(root_sk, &payload)?,
        })
    }
}

impl RootSigned for DeviceCert {
    const ACCOUNT_MISMATCH: AccountError = AccountError::CertAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::CertSignatureInvalid;

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
            &self.sign_pk,
            &self.kem_pk,
            self.key_epoch,
            self.device_epoch,
        )
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

/// A [`DeviceCert`] whose genesis anchor, key chain, and signature have all been
/// checked. See [`Verified`] for what that does — and does not — mean.
pub type VerifiedDeviceCert = Verified<DeviceCert>;

/// Verify a device certificate end to end against a self-certifying genesis.
///
/// Takes the anchor and chain separately rather than an
/// [`crate::AccountProof`] so a caller holding a borrowed chain verifies without
/// allocating one; a caller that *has* a proof should call
/// [`crate::AccountProof::verify`] instead.
///
/// # Errors
/// See [`verify_root_signed`]; every [`AccountError`] variant it can report is
/// reachable from here.
pub fn verify_device_cert(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, AccountError> {
    verify_root_signed(claimed_account, genesis, chain, cert)?;
    Ok(Verified::new(*cert))
}
