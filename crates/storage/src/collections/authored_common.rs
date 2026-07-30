//! Shared author-tracking mechanics for [`AuthoredMap`](super::authored_map::AuthoredMap)
//! and [`AuthoredVector`](super::authored_vector::AuthoredVector): sourcing the
//! current writer as the owner, constructing the per-entry owner stamp, and
//! the owner-gate accept/reject check. The collection-specific method shapes
//! (key vs index, reject-on-collision vs slot-return, delete vs tombstone) live
//! in each collection's own file.
//!
//! The owner is the **device**, not the account. Authorship here is a claim about
//! which replica wrote an entry, and the gate it feeds ("only the author may
//! update this") is enforced against a stored `PublicKey`. Re-keying it onto an
//! account would let one device edit an entry another device of the same account
//! wrote — a different feature, and one that needs the writer-set plumbing rather
//! than a swapped id.

use calimero_primitives::identity::PublicKey;

use crate::entities::StorageType;
use crate::env;

/// Return the current writer as a `PublicKey` — the value that gets
/// stamped onto every new author-tracked entry.
pub(super) fn current_writer() -> PublicKey {
    env::device_id().into()
}

/// Build the `StorageType::User { owner }` stamp for the current writer.
/// Called by `AuthoredMap::insert` and `AuthoredVector::push`.
pub(super) fn make_owner_stamp() -> StorageType {
    StorageType::User {
        owner: current_writer(),
        signature_data: None,
    }
}

/// Predicate: the current writer matches `owner`.
/// Called by every gated mutation (`update`, `remove`, `tombstone`).
pub(super) fn writer_matches_owner(owner: &PublicKey) -> bool {
    &current_writer() == owner
}

// `with_device_id` / `set_device_id` are gated
// `#[cfg(not(target_arch = "wasm32"))]`, so tests that mutate the executor
// identity must match that gate to keep the `cargo test --target wasm32-*`
// build green.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn current_writer_returns_env_device_id() {
        env::with_device_id([42; 32], || {
            let expected: PublicKey = [42; 32].into();
            assert_eq!(current_writer(), expected);
        });
    }

    #[test]
    #[serial]
    fn make_owner_stamp_uses_current_executor() {
        env::with_device_id([42; 32], || match make_owner_stamp() {
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
        env::with_device_id([42; 32], || {
            let owner: PublicKey = [42; 32].into();
            assert!(writer_matches_owner(&owner));
        });
    }

    #[test]
    #[serial]
    fn owner_check_rejects_mismatched_executor() {
        env::with_device_id([42; 32], || {
            let owner: PublicKey = [99; 32].into();
            assert!(!writer_matches_owner(&owner));
        });
    }
}
