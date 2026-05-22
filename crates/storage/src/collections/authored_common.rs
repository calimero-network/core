//! Shared author-tracking primitives for [`AuthoredMap`](super::authored_map::AuthoredMap)
//! and [`AuthoredVector`](super::authored_vector::AuthoredVector).
//!
//! Both collections stamp each entry with the current executor identity at
//! write time and reject non-owner mutations. The signatures of the
//! collection-specific methods differ enough (map vs sequence) that a single
//! generic wrapper would still need bespoke impl blocks per shape — keying
//! by `K` vs by index, reject-on-collision vs auto-slot-return, physical
//! delete vs in-place tombstone with `V: Default` — so the right factoring
//! is to extract the *author-tracking mechanics* (executor lookup, owner
//! stamp construction, owner-equality check) into this shared helper module
//! while the public method shapes stay distinct.
//!
//! This module owns the **identical** part: how the owner is sourced, how
//! the stamp is constructed, and how the owner-gate check decides accept vs
//! reject. The collection-specific methods (`AuthoredMap::insert` rejects on
//! collision; `AuthoredVector::push` returns the assigned slot; `tombstone`
//! is slot-preserving) stay in their respective files because their shapes
//! cannot collapse.

use calimero_primitives::identity::PublicKey;

use crate::entities::StorageType;
use crate::env;

/// Return the current executor as a `PublicKey` — the value that gets
/// stamped onto every new author-tracked entry.
pub(super) fn current_executor() -> PublicKey {
    env::executor_id().into()
}

/// Build the `StorageType::User { owner }` stamp for the current executor.
/// Called by `AuthoredMap::insert` and `AuthoredVector::push`.
pub(super) fn make_owner_stamp() -> StorageType {
    StorageType::User {
        owner: current_executor(),
        signature_data: None,
    }
}

/// Predicate: the current executor matches `owner`.
/// Called by every gated mutation (`update`, `remove`, `tombstone`).
pub(super) fn executor_matches_owner(owner: &PublicKey) -> bool {
    &current_executor() == owner
}

// `with_executor_id` / `set_executor_id` are gated
// `#[cfg(not(target_arch = "wasm32"))]`, so tests that mutate the executor
// identity must match that gate to keep the `cargo test --target wasm32-*`
// build green.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn current_executor_returns_env_executor_id() {
        env::with_executor_id([42; 32], || {
            let expected: PublicKey = [42; 32].into();
            assert_eq!(current_executor(), expected);
        });
    }

    #[test]
    #[serial]
    fn make_owner_stamp_uses_current_executor() {
        env::with_executor_id([42; 32], || match make_owner_stamp() {
            StorageType::User {
                owner,
                signature_data,
            } => {
                let expected: PublicKey = [42; 32].into();
                assert_eq!(owner, expected);
                assert!(
                    signature_data.is_none(),
                    "freshly-built stamp must not carry signature_data"
                );
            }
            other => panic!("expected StorageType::User, got {other:?}"),
        });
    }

    #[test]
    #[serial]
    fn owner_check_accepts_matching_executor() {
        env::with_executor_id([42; 32], || {
            let owner: PublicKey = [42; 32].into();
            assert!(executor_matches_owner(&owner));
        });
    }

    #[test]
    #[serial]
    fn owner_check_rejects_mismatched_executor() {
        env::with_executor_id([42; 32], || {
            let owner: PublicKey = [99; 32].into();
            assert!(!executor_matches_owner(&owner));
        });
    }
}
