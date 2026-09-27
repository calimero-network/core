//! The namespace id an account founds: a hash of the founder and a salt.
//!
//! # Why it is shaped this way
//!
//! A namespace id used to be 32 random bytes, related to nothing. That left the
//! genesis op (`RootOp::NamespaceCreated { founder }`) authoritative only within
//! a DAG a replica already trusts: the id itself could not say which founder is
//! the real one, so anybody could sign a self-consistent genesis naming
//! themselves for an id they did not create (#2932).
//!
//! Deriving the id from the founder's [`AccountId`] makes the id the root of
//! trust. `domain_hash(NAMESPACE_ID_DOMAIN, [founder, salt])` can be recomputed
//! by anyone holding the founder and the salt, and matching it for somebody
//! else's id would mean finding a preimage of SHA-256: an account can only ever
//! found the ids its own account id hashes to.
//!
//! The salt is what lets one account found many namespaces. It is not a secret
//! and it adds nothing to the binding; the founder is what the id commits to.
//!
//! This module only defines the derivation. Who records the salt, and who
//! checks it, is the caller's business: the founding node keeps it, and a
//! relying party that is shown `(founder, salt)` can confirm an account founded
//! a namespace without holding any of its governance state.

use calimero_primitives::identity::{domain_hash, AccountId};

use crate::domain::NAMESPACE_ID_DOMAIN;

/// Bytes of salt a founded namespace id is derived with.
pub const NAMESPACE_SALT_LEN: usize = 32;

/// The id of the namespace `founder` founds with `salt`.
#[must_use]
pub fn founded_namespace_id(founder: &AccountId, salt: &[u8; NAMESPACE_SALT_LEN]) -> [u8; 32] {
    domain_hash(NAMESPACE_ID_DOMAIN, &[founder.as_bytes(), salt])
}

/// Whether `namespace_id` is the one `founder` founds with `salt`.
///
/// `false` for every namespace whose id was not derived this way, which
/// includes every namespace created before derivation existed: their ids are
/// random, so no founder and salt reproduce them.
#[must_use]
pub fn is_founded_by(
    namespace_id: &[u8; 32],
    founder: &AccountId,
    salt: &[u8; NAMESPACE_SALT_LEN],
) -> bool {
    founded_namespace_id(founder, salt) == *namespace_id
}
