//! Ordered shared-keyspace map with per-entry ownership.
//!
//! [`AuthoredSortedMap<K, V>`] is to [`AuthoredMap`](super::authored_map::AuthoredMap)
//! what [`SortedMap`](super::SortedMap) is to [`UnorderedMap`](super::UnorderedMap):
//! the same entries, the same owner stamp, the same merge — plus an ordered
//! view, so a reader can seek to a range or a key prefix instead of walking
//! the whole collection.
//!
//! # Why this exists
//!
//! `AuthoredMap` has no ordered read at all: `entries()` is the only iteration
//! and it loads every entry in the collection. For an app whose keys are
//! hierarchical — `"<game>/<ply>/<account>/<nonce>"`, `"<room>/<ts>/<author>"`,
//! `"<doc>/<section>/<editor>"` — that turns "read one slice" into "read
//! everything", and the slice is usually a rounding error next to the whole.
//!
//! That is a **liveness** problem specific to authored data, not just a
//! performance one, and the two halves compound:
//!
//! * Any context member may INSERT under any key. Insert is open by design —
//!   that is what makes the keyspace shared.
//! * Only an entry's own owner may ever REMOVE it. So entries written by
//!   somebody acting in bad faith cannot be cleaned up by anyone else, ever.
//!
//! Put together: a member can grow the collection without bound, nobody can
//! shrink it, and with only a full scan available every honest reader pays for
//! every junk entry on every read, forever. The entries need not be *believed*
//! to do damage — an app that correctly ignores all of them still reads them
//! all. Correct answers, unusable app.
//!
//! An ordered index turns that back into `O(log n + k)`: a reader pays for the
//! `k` entries under the prefix it asked for and nothing for the rest. A
//! targeted writer can still crowd one specific prefix, which no collection can
//! prevent, but the blast radius of an untargeted flood drops to zero.
//!
//! # What it costs
//!
//! Exactly what `SortedMap` costs, and for the same reason: a node-local,
//! derived, **non-synced** secondary index, maintained on every `insert` /
//! `remove` and rebuilt once after a remote sync (which mutates entries
//! host-side without going through those methods). So:
//!
//! * writes pay an extra index write and a marker read/write,
//! * the node stores a little more per key,
//! * the first ordered read after a sync is `O(n)`.
//!
//! **Default to [`AuthoredMap`](super::authored_map::AuthoredMap).** Reach for
//! this one when the keys are hierarchical and reads are slices of them — which
//! is also exactly when the flood above is worth defending against.
//!
//! # Merge semantics
//!
//! Identical to `AuthoredMap`, and deliberately indistinguishable on the wire:
//! the ordering is derived from the key set by each node for itself, so there
//! is nothing extra to replicate. Entries carry `StorageType::User { owner }`,
//! per-entry authorization is enforced at apply in `Interface::apply_action`,
//! and the container reports [`CrdtType::UserStorage`] — the same variant
//! `AuthoredMap` reports, on purpose. A new `CrdtType` would have been a borsh
//! discriminant change for a distinction the sync layer must not make: two
//! nodes holding the same entries, one using each collection, have to agree on
//! the root hash, and they do.

use super::{Authored, SortedMap};
use crate::store::MainStorage;

/// A map keyed by `K`, iterated in ascending key order, where each entry is
/// owned by the account that inserted it.
///
/// `Authored<SortedMap<K, V>>`, under its original name and with its original
/// bytes. The ordered reads (`range`, `prefix`, `page`, `keys`) are the inner
/// `SortedMap`'s. See [`Guarded`](super::Guarded) for the writes.
pub type AuthoredSortedMap<K, V, S = MainStorage> = Authored<SortedMap<K, V, S>>;

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use serial_test::serial;

    use super::AuthoredSortedMap;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    /// The tests name the OWNER, and an owner is an account.
    fn acct(bytes: [u8; 32]) -> AccountId {
        AccountId::from(bytes)
    }

    fn map() -> Root<AuthoredSortedMap<String, u64>> {
        Root::new(AuthoredSortedMap::<String, u64>::new)
    }

    #[test]
    fn new_plus_reassign_is_convergent() {
        // Wrapper type: `new_with_field_name` leaves the wrapper id random and
        // only the inner map deterministic; `reassign` canonicalises the
        // wrapper too. The CIP-I9 property is convergence — two independent
        // `new() + reassign("f")` mint the same id.
        env::reset_for_testing();
        let mut a: AuthoredSortedMap<String, u32> = AuthoredSortedMap::new();
        a.reassign_deterministic_id("items");
        let mut b: AuthoredSortedMap<String, u32> = AuthoredSortedMap::new();
        b.reassign_deterministic_id("items");
        assert_eq!(
            <AuthoredSortedMap<String, u32> as crate::entities::Data>::id(&a),
            <AuthoredSortedMap<String, u32> as crate::entities::Data>::id(&b),
        );
    }

    // ── the owner gate, which must behave exactly as AuthoredMap's ──────────

    #[test]
    #[serial]
    fn insert_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        map.insert("apple".to_owned(), 1).expect("insert");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );
        assert_eq!(map.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn insert_rejects_existing_key() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();

        let err = map
            .insert("apple".to_owned(), 2)
            .expect_err("insert on existing key must fail");
        assert!(
            err.to_string().to_lowercase().contains("already"),
            "error should mention key already exists, got: {err}"
        );
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
    }

    #[test]
    #[serial]
    fn a_second_account_cannot_take_an_occupied_key() {
        // The squat case the other way round: BOB cannot claim ALICE's key by
        // inserting over it, and cannot update or remove it either. Insert is
        // open; an OCCUPIED key is not.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_account_id(BOB);
        assert!(map.insert("apple".to_owned(), 2).is_err());
        assert!(map.update(&"apple".to_owned(), 99).is_err());
        assert!(map.remove(&"apple".to_owned()).is_err());

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );
    }

    #[test]
    #[serial]
    fn update_and_remove_by_owner_succeed() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();
        map.update(&"apple".to_owned(), 42).expect("owner update");
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(42));
        // The stamp survives an update — an owner editing their own entry must
        // not look like a fresh insert by whoever happens to be executing.
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );

        assert_eq!(map.remove(&"apple".to_owned()).unwrap(), Some(42));
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn a_second_device_of_the_owning_account_may_still_write() {
        // Authorship is the ACCOUNT, not the device: a laptop editing what the
        // phone wrote is the same person.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        env::set_device_id([0xD1; 32]);
        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_device_id([0xD2; 32]);
        map.update(&"apple".to_owned(), 7)
            .expect("a second device of the owner must be able to write");
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(7));
    }

    // ── the ordered reads, which are the reason this type exists ────────────

    #[test]
    #[serial]
    fn entries_and_keys_come_back_in_key_order() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["c", "a", "b"] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(keys, vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
    }

    #[test]
    #[serial]
    fn prefix_returns_only_the_slice_asked_for() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in [
            "game/0000/alice/1",
            "game/0000/alice/2",
            "game/0001/bob/1",
            "other/0000/alice/1",
        ] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let hits: Vec<String> = map.prefix(b"game/0000/").unwrap().map(|(k, _)| k).collect();
        assert_eq!(
            hits,
            vec![
                "game/0000/alice/1".to_owned(),
                "game/0000/alice/2".to_owned()
            ]
        );

        // A prefix that narrows to one author is the shape the owner gate makes
        // worth having: the entries it returns are the only ones whose stamp
        // then has to be checked.
        let bobs: Vec<String> = map
            .prefix(b"game/0001/bob/")
            .unwrap()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(bobs, vec!["game/0001/bob/1".to_owned()]);

        assert!(map.prefix(b"nothing/").unwrap().next().is_none());
    }

    #[test]
    #[serial]
    fn a_prefix_read_is_unmoved_by_entries_outside_it() {
        // The property the collection exists for, stated as behaviour rather
        // than as a timing: entries piled up elsewhere in the keyspace — which
        // any member may write and only their own author may ever remove —
        // do not appear in, and do not enlarge, another prefix's result.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        let mut map = map();
        map.insert("game/0000/alice/1".to_owned(), 1).unwrap();

        env::set_account_id(BOB);
        for i in 0..200u64 {
            map.insert(format!("game/0500/bob/{i}"), i).unwrap();
        }

        env::set_account_id(ALICE);
        let hits: Vec<String> = map.prefix(b"game/0000/").unwrap().map(|(k, _)| k).collect();
        assert_eq!(hits, vec!["game/0000/alice/1".to_owned()]);
        assert_eq!(map.len().unwrap(), 201);
    }

    #[test]
    #[serial]
    fn range_and_page_read_in_order() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["a", "b", "c", "d", "e"] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let in_range: Vec<String> = map
            .range("b".to_owned().."d".to_owned())
            .unwrap()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(in_range, vec!["b".to_owned(), "c".to_owned()]);

        let page: Vec<String> = map
            .page(1, 2)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(page, vec!["b".to_owned(), "c".to_owned()]);
    }

    #[test]
    #[serial]
    fn removing_an_entry_takes_it_out_of_the_ordered_reads() {
        // The index is maintained on remove, not only on insert — a stale index
        // would keep serving a key whose entry is gone.
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["a", "b", "c"] {
            map.insert(k.to_owned(), 1).unwrap();
        }
        assert_eq!(map.remove(&"b".to_owned()).unwrap(), Some(1));

        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(keys, vec!["a".to_owned(), "c".to_owned()]);
        assert!(map.prefix(b"b").unwrap().next().is_none());
    }
}
