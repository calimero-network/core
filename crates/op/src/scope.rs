//! What a scope *is*, and the one number that says two replicas of it agree.
//!
//! A scope is the unit of everything: replication, encryption, membership, and
//! convergence. There is no cross-scope root — [`scope_root`] summarises one
//! scope's whole projection and nothing else — so the id that names a scope and
//! the hash that compares two copies of it belong in the same place.

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

/// Stable id of a **visibility scope** — one node in a context's scope tree
/// (root governance scope, a context, a subgroup, …). Each scope is a
/// self-contained replication + encryption + convergence domain with its own
/// op-log, key, members, and [`scope_root`]. Convergence is always per scope.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct ScopeId([u8; 32]);

impl ScopeId {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for ScopeId {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

/// The single convergence root over a scope's **whole** projection —
/// values **and** authorization (ACL + groups). Folding the ACL/membership in
/// is what makes a hash-neutral writer/membership rotation impossible to hide:
/// a divergent writer set is a divergent root, so sync can never declare
/// "done" while the authorization state disagrees.
///
/// Combining function only; `calimero-projection` computes the three component
/// hashes from a `ScopeState`.
#[must_use]
pub fn scope_root(entities_root: [u8; 32], acl_hash: [u8; 32], groups_root: [u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(entities_root);
    hasher.update(acl_hash);
    hasher.update(groups_root);
    hasher.finalize().into()
}
