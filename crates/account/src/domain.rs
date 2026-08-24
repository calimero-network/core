//! Every signing and content-address domain this crate uses, in one place.
//!
//! # Why it is shaped this way
//!
//! They are collected here rather than declared beside their users because the
//! property that matters is a property of the *set*: any two that collide would
//! let a signature minted for one purpose be replayed as another. Keeping them
//! together is what makes `signing_domains_are_pairwise_distinct` a check over
//! the whole crate instead of over whichever module happened to be in view.
//!
//! [`PAIRING_CONFIRMATION_HEX_LEN`] is the one non-domain here, and it sits
//! beside the domain it belongs to: the confirmation code's length is the work
//! factor its separated domain exists to protect.

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

/// Domain for an author's warrant delegating one intent to one executor.
///
/// Distinct from every other domain here for the usual reason, and from
/// [`DEVICE_CERT_SIGN_DOMAIN`] for a sharper one: a warrant is signed by a
/// DEVICE key while a certificate is signed by a ROOT key, so a shared domain
/// would let a device that holds neither role sign bytes the other would accept.
pub(crate) const WARRANT_SIGN_DOMAIN: &[u8] = b"calimero.warrant.v1";

/// Number of hex characters in a [`crate::PairingOffer::confirmation_code`],
/// excluding its separators. Eight bytes of digest.
pub(crate) const PAIRING_CONFIRMATION_HEX_LEN: usize = 16;

/// Domain for the hash a warrant commits to instead of the intent itself.
///
/// Distinct from [`WARRANT_SIGN_DOMAIN`] because the two are different jobs on
/// the same values: this one produces the commitment, that one signs over it.
/// Sharing a domain would make the commitment a truncated disclosure of bytes
/// something signs.
pub(crate) const WARRANT_INTENT_DOMAIN: &[u8] = b"calimero.warrant.intent.v1";

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
    WARRANT_SIGN_DOMAIN,
    WARRANT_INTENT_DOMAIN,
];
