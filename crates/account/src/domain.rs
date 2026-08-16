//! Every signing and content-address domain this crate uses, in one place.
//!
//! They are collected here rather than declared beside their users because the
//! property that matters is a property of the *set*: any two that collide would
//! let a signature minted for one purpose be replayed as another. Keeping them
//! together is what makes `signing_domains_are_pairwise_distinct` a check over
//! the whole crate instead of over whichever module happened to be in view.

use borsh::BorshSerialize;

/// Domain separator for the [`crate::AccountId`] content address.
pub(crate) const ACCOUNT_ID_DOMAIN: &[u8] = b"calimero.account.genesis.v1";
/// Domain separator for the bytes a root key signs to hand off to its successor.
pub(crate) const HANDOFF_SIGN_DOMAIN: &[u8] = b"calimero.account.handoff.v1";
/// Domain separator for the bytes a root key signs to grant a device.
pub(crate) const DEVICE_CERT_SIGN_DOMAIN: &[u8] = b"calimero.device.cert.v1";

/// Domain for a member's endorsement of an account. Distinct from every other
/// signing domain, so an endorsement signature can never be replayed as a device
/// certificate, a handoff, or an op.
pub(crate) const ACCOUNT_ENDORSEMENT_SIGN_DOMAIN: &[u8] = b"calimero.account.endorsement.v1";

/// Domain for a root-signed device revocation.
pub(crate) const DEVICE_REVOCATION_SIGN_DOMAIN: &[u8] = b"calimero.device.revocation.v1";

/// Domain for a pairing device's statement over the key material it minted.
pub(crate) const PAIRING_STATEMENT_SIGN_DOMAIN: &[u8] = b"calimero.device.pairing.v1";

/// Domain for the pairing confirmation code. Separate from the statement's
/// signing domain because the code is a value humans read aloud, not a
/// signature preimage — sharing a domain would make the code a truncated
/// disclosure of bytes something signs over.
pub(crate) const PAIRING_CONFIRMATION_DOMAIN: &[u8] = b"calimero.device.pairing.confirm.v1";

/// Every signing domain used by this crate, for the test that asserts they are
/// pairwise distinct. A collision here would let a signature minted for one
/// purpose be replayed as another.
#[cfg(test)]
pub(crate) const ALL_DOMAINS: &[&[u8]] = &[
    ACCOUNT_ID_DOMAIN,
    calimero_primitives::identity::DEVICE_ID_DOMAIN,
    HANDOFF_SIGN_DOMAIN,
    DEVICE_CERT_SIGN_DOMAIN,
    ACCOUNT_ENDORSEMENT_SIGN_DOMAIN,
    DEVICE_REVOCATION_SIGN_DOMAIN,
    PAIRING_STATEMENT_SIGN_DOMAIN,
    PAIRING_CONFIRMATION_DOMAIN,
];

/// Serialize with borsh into a `Vec<u8>`.
///
/// **Deliberately not fallible, even though the signing helpers that call it
/// return `Result`.** Its other callers are the content addresses —
/// [`crate::AccountGenesis::account_id`], [`crate::DeviceId::mint`], every
/// `payload()` — and an id computation that can fail is a worse API than a panic
/// that cannot happen: it would put a `Result` on the most-called function in
/// this crate to model an outcome no input can produce. The failure the signing
/// helpers *can* have is the signer refusing, and that is what
/// [`crate::AccountError::SigningFailed`] is.
///
/// # Panics
/// Never: every type passed here is fixed-size plain data, and a `Vec` writer has
/// no failure mode, so `borsh::to_vec` has nothing to fail on.
pub(crate) fn borsh_bytes<T: BorshSerialize>(value: &T) -> Vec<u8> {
    borsh::to_vec(value).expect("borsh serialization of a plain-data type is infallible")
}
