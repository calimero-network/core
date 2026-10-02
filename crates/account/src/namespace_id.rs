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
//! The salt is what lets one account found many namespaces. It adds nothing to
//! the binding; the founder is what the id commits to. An ordinary namespace
//! draws it at random and need not keep it secret. The account namespace
//! derives it from the account root's secret instead, so every device of the
//! account computes the same id and nobody holding only the account id can.
//!
//! Every namespace id is derived this way: the genesis carries the salt, and a
//! genesis whose pair does not reproduce the id founds nothing.
//!
//! This module only defines the derivation. Who records the salt, and who
//! checks it, is the caller's business: the founding node keeps it, and a
//! relying party that is shown `(founder, salt)` can confirm an account founded
//! a namespace without holding any of its governance state.

use calimero_primitives::identity::{domain_hash, AccountId};

use crate::domain::{NAMESPACE_ID_DOMAIN, SUBGROUP_ID_DOMAIN};

/// Bytes of salt a founded namespace id is derived with.
pub const NAMESPACE_SALT_LEN: usize = 32;

/// The id of the namespace `founder` founds with `salt`.
#[must_use]
pub fn founded_namespace_id(founder: &AccountId, salt: &[u8; NAMESPACE_SALT_LEN]) -> [u8; 32] {
    domain_hash(NAMESPACE_ID_DOMAIN, &[founder.as_bytes(), salt])
}

/// Whether `namespace_id` is the one `founder` founds with `salt`.
///
/// `false` for every pair that does not reproduce the id, which is every pair
/// but the founder's own.
#[must_use]
pub fn is_founded_by(
    namespace_id: &[u8; 32],
    founder: &AccountId,
    salt: &[u8; NAMESPACE_SALT_LEN],
) -> bool {
    founded_namespace_id(founder, salt) == *namespace_id
}

/// Bytes of salt a created subgroup id is derived with.
pub const SUBGROUP_SALT_LEN: usize = 32;

/// The id of the subgroup `creator` creates under `parent`, with birth
/// visibility `restricted`, and `salt`.
///
/// A subgroup id is bound to its create the way a namespace id is bound to its
/// founder, and for a sharper reason: a subgroup's `RootOp::GroupCreated` can be
/// raced. Once an id is visible (gossip of the genuine op, or anything that
/// names it), another member with create authority could sign its own create
/// for the same id before the genuine one is in its causal frontier. The two
/// ops are then concurrent, replicas fold them in either order, and whichever
/// landed first would own the group on that replica, permanently.
///
/// Deriving the id from everything the create establishes makes that
/// impossible, not just resolvable: a create for an id is valid only if its
/// `(creator, parent, restricted, salt)` reproduces it, so any two valid creates
/// for one id name the same creator, the same parent and the same birth
/// visibility, and folding them in any order gives the same group. Naming
/// someone else's id would mean finding a SHA-256 preimage.
///
/// `domain_hash("calimero.subgroup.id.v1", [creator, parent, [restricted as
/// u8], salt])`, every part length-prefixed. The salt is random and need not be
/// kept; the op carries it.
#[must_use]
pub fn created_subgroup_id(
    creator: &AccountId,
    parent: &[u8; 32],
    restricted: bool,
    salt: &[u8; SUBGROUP_SALT_LEN],
) -> [u8; 32] {
    domain_hash(
        SUBGROUP_ID_DOMAIN,
        &[creator.as_bytes(), parent, &[u8::from(restricted)], salt],
    )
}
