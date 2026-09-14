//! The bytes a device signs to log in to a node.
//!
//! A node issues a single-use challenge and the device signs this digest over
//! it. Binding the device's own key into the digest means a signature cannot be
//! presented for a different key, and the separate domain means it can never be
//! mistaken for a warrant or a certificate.

use calimero_primitives::identity::{domain_hash, PublicKey};

use crate::domain::DEVICE_LOGIN_SIGN_DOMAIN;

/// The 32-byte digest `device_key` signs to log in with `challenge`.
#[must_use]
pub fn device_login_payload(challenge: &[u8; 32], device_key: &PublicKey) -> [u8; 32] {
    let key: &[u8; 32] = device_key.as_ref();
    domain_hash(DEVICE_LOGIN_SIGN_DOMAIN, &[&challenge[..], &key[..]])
}
