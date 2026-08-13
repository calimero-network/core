//! Shared author-tracking mechanics for [`AuthoredMap`](super::authored_map::AuthoredMap)
//! and [`AuthoredVector`](super::authored_vector::AuthoredVector): sourcing the
//! current writer as the owner, constructing the per-entry owner stamp, and
//! the owner-gate accept/reject check. The collection-specific method shapes
//! (key vs index, reject-on-collision vs slot-return, delete vs tombstone) live
//! in each collection's own file.
//!
//! The owner is the **account**, not the device. Authorship here feeds an
//! access-control gate ("only the author may change this entry"), and access
//! control names people: someone who wrote an entry from their phone can edit it
//! from their laptop, because both devices speak for one account. Keying the gate
//! on a device key made a second device of the same person a stranger to their
//! own data.
//!
//! This is the authorization half only. The per-replica identities CRDT mechanics
//! depend on — an LWW register's tiebreak, a counter's per-actor slot, an HLC
//! seed — stay on [`env::device_id`] and must: two devices of one account writing
//! concurrently have to remain distinguishable or they share a slot and lose each
//! other's writes. Which device wrote an entry is still recorded, as the
//! `signature_data.signer` its signature verifies against.

use calimero_account::AccountId;

use crate::entities::StorageType;
use crate::env;

/// Return the current writer as an `AccountId` — the value that gets
/// stamped onto every new author-tracked entry.
pub(super) fn current_writer() -> AccountId {
    AccountId::from(env::account_id())
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
///
/// This is the local, pre-write half of the gate — it answers for the account
/// executing right now. The authoritative half runs at apply, where a remote
/// write's signer is resolved to its account at the action's causal cut and
/// compared against the stored `owner`.
pub(super) fn writer_matches_owner(owner: &AccountId) -> bool {
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
    fn current_writer_returns_env_account_id() {
        env::with_account_id([42; 32], || {
            assert_eq!(current_writer(), AccountId::from([42; 32]));
        });
    }

    #[test]
    #[serial]
    fn make_owner_stamp_uses_current_account() {
        env::with_account_id([42; 32], || match make_owner_stamp() {
            StorageType::User {
                owner,
                signature_data,
            } => {
                assert_eq!(owner, AccountId::from([42; 32]));
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
    fn owner_check_accepts_matching_account() {
        env::with_account_id([42; 32], || {
            assert!(writer_matches_owner(&AccountId::from([42; 32])));
        });
    }

    #[test]
    #[serial]
    fn owner_check_rejects_a_different_account() {
        env::with_account_id([42; 32], || {
            assert!(!writer_matches_owner(&AccountId::from([99; 32])));
        });
    }

    /// The whole point of the change: one person, two machines.
    ///
    /// Fails before authorship moved onto accounts — the stamp was the device id,
    /// so moving the device moved the owner and the second machine was refused
    /// its own entry.
    #[test]
    #[serial]
    fn a_second_device_of_the_same_account_still_owns_the_entry() {
        let stamped =
            env::with_device_id([1; 32], || env::with_account_id([42; 32], make_owner_stamp));
        let StorageType::User { owner, .. } = stamped else {
            panic!("expected StorageType::User");
        };

        // Same person, different machine. Only the device moves.
        env::with_device_id([2; 32], || {
            env::with_account_id([42; 32], || {
                assert!(
                    writer_matches_owner(&owner),
                    "a second device of the owning account must be able to edit the entry"
                );
            });
        });
    }

    /// The guard on the test above: "any device may edit" would satisfy it too.
    #[test]
    #[serial]
    fn a_device_of_another_account_does_not_own_the_entry() {
        let stamped =
            env::with_device_id([1; 32], || env::with_account_id([42; 32], make_owner_stamp));
        let StorageType::User { owner, .. } = stamped else {
            panic!("expected StorageType::User");
        };

        // Same machine id as the owner's, deliberately: it is the ACCOUNT that
        // must decide, so holding the device fixed and moving the account is the
        // only way to prove the device is not what is being read.
        env::with_device_id([1; 32], || {
            env::with_account_id([99; 32], || {
                assert!(
                    !writer_matches_owner(&owner),
                    "a different account must not be able to edit someone else's entry"
                );
            });
        });
    }
}
