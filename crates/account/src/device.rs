//! Device credentials: the two keys a device holds, and the root-signed
//! [`DeviceCert`] that binds them to an account.
//!
//! Verification here is *internal* validity only — see [`VerifiedDeviceCert`]
//! for what that deliberately does not mean.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey, PublicKey};

use crate::account::AccountGenesis;
use crate::domain::DEVICE_CERT_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::root_key::{root_key_at_epoch, RootKeyHandoff};

/// An X25519 public key used only as a scope-key delivery recipient.
///
/// Deliberately a distinct type from [`PublicKey`] (Ed25519). A device carries
/// **two** keys — one that signs, one that receives wrapped keys — rather than
/// reusing a single Ed25519 key for both signing and key agreement. Single-key
/// dual-use across a signature scheme and a Diffie-Hellman is a well-known
/// footgun with no compensating benefit, and the type split makes it impossible
/// to pass one where the other belongs.
///
/// This crate only carries the bytes; the wrapping itself lives with scope-key
/// delivery.
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
///
/// There is deliberately **no expiry field**. Expiry requires participants to
/// agree on wall-clock time, which a causally-ordered system does not provide;
/// a certificate that expires "at" some timestamp would be valid on one node
/// and invalid on another, and authorization would stop converging. Withdrawal
/// is expressed as a revocation op instead, which is causally ordered like
/// every other decision.
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

    /// The bytes this certificate's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            self.account,
            self.device,
            &self.sign_pk,
            &self.kem_pk,
            self.key_epoch,
            self.device_epoch,
        )
    }
}

/// Mint a device certificate, signed by the account root key at `key_epoch`.
///
/// Same reasoning as [`crate::sign_root_key_handoff`]: one place assembles the
/// preimage, so signer and verifier cannot drift.
///
/// `device_epoch` must strictly exceed any epoch already folded for this device
/// — the projection refuses a link that does not advance it, so a re-issued
/// certificate that reuses an epoch is inert rather than a rollback.
///
/// Takes one parameter per signed field rather than a builder: the argument
/// list *is* the list of fields the signature covers, and hiding it behind
/// optional setters would make an unsigned field easy to miss.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub fn sign_device_cert(
    root_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    sign_pk: &PublicKey,
    kem_pk: &KemPublicKey,
    key_epoch: u32,
    device_epoch: u32,
) -> Result<DeviceCert, AccountError> {
    let payload =
        DeviceCert::signing_payload(account, device, sign_pk, kem_pk, key_epoch, device_epoch);
    Ok(DeviceCert {
        account,
        device,
        sign_pk: *sign_pk,
        kem_pk: *kem_pk,
        key_epoch,
        device_epoch,
        signature: root_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// A [`DeviceCert`] whose genesis anchor, key chain, and signature have all
/// been checked.
///
/// Holding one means the credential is **internally** valid. It does **not**
/// mean the binding is currently in force: whether the signing epoch has since
/// been superseded, whether the device was revoked, and whether the account is
/// even a member of the scope are all at-cut questions that only the projection
/// can answer. Those checks are the projection's, and this type exists so the
/// two stages can't be confused for one another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedDeviceCert {
    /// Account the device is bound to.
    pub account: AccountId,
    /// The bound device.
    pub device: DeviceId,
    /// The device's op-signing key.
    pub sign_pk: PublicKey,
    /// The device's scope-key delivery key.
    pub kem_pk: KemPublicKey,
    /// Root-key epoch that signed the certificate.
    pub key_epoch: u32,
    /// The device's key-rotation epoch.
    pub device_epoch: u32,
}

/// Verify a device certificate end to end against a self-certifying genesis.
///
/// Checks, in order: the genesis addresses `claimed_account`; the certificate
/// is for that same account; the handoff chain is valid and reaches the
/// certificate's epoch; and the certificate is signed by the root key at that
/// epoch.
///
/// The `claimed_account` parameter is what ties the whole credential to
/// something the caller already trusts — typically `op.author`. Without it a
/// caller could verify a perfectly well-formed credential for an account nobody
/// asked about.
///
/// # Errors
/// See [`AccountError`]; every variant is reachable from this function.
pub fn verify_device_cert(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, AccountError> {
    let derived = genesis.account_id();
    if derived != claimed_account {
        return Err(AccountError::GenesisMismatch {
            claimed: claimed_account,
            actual: derived,
        });
    }
    if cert.account != derived {
        return Err(AccountError::CertAccountMismatch);
    }

    let signer = root_key_at_epoch(genesis, chain, cert.key_epoch)?;

    if signer
        .verify_raw_signature(&cert.payload(), &cert.signature)
        .is_err()
    {
        return Err(AccountError::CertSignatureInvalid);
    }

    Ok(VerifiedDeviceCert {
        account: cert.account,
        device: cert.device,
        sign_pk: cert.sign_pk,
        kem_pk: cert.kem_pk,
        key_epoch: cert.key_epoch,
        device_epoch: cert.device_epoch,
    })
}
