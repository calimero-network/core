//! Map with per-owner keys.
//!
//! `AuthoredMap<K, V>` exposes an `UnorderedMap<K, V>` whose entries each carry
//! a `StorageType::User { owner }` stamp set to the inserter's ACCOUNT. Every
//! account has its own namespace: two accounts inserting one key hold two
//! entries. Only the owning account can update or remove its entries — from any
//! of its devices. Reads are unrestricted; the key-only ones read the caller's
//! own entry and `get_by` names another owner.
//!
//! The per-entry authorization is enforced at merge time in
//! `Interface::apply_action` (see `interface.rs`). Local `update`/`remove`
//! additionally short-circuit non-owner calls so bugs surface in-process.
//!
//! # Merge semantics
//!
//! The owner stamp travels with each entry, and every node checks it when it
//! applies a peer's upsert or delete, so per-entry authorization survives sync.
//! New keys from either side are unioned; updates to the same key resolve
//! through the entry's own merge (typically LWW on the contained value). The
//! container never merges structurally: see [`Guarded`](super::Guarded).

use super::{Authored, UnorderedMap};
use crate::store::MainStorage;

/// A map keyed by `K` where each entry is owned by the account that inserted it.
///
/// `Authored<UnorderedMap<K, V>>`, under its original name and with its
/// original bytes. Each entry's `StorageType` is `User { owner }`, set at
/// insert time from `env::account_id()`. Only that account can `update` or
/// `remove` the entry, from any device it holds. See [`Guarded`](super::Guarded)
/// for the methods.
pub type AuthoredMap<K, V, S = MainStorage> = Authored<UnorderedMap<K, V, S>>;

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use serial_test::serial;

    use super::AuthoredMap;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    #[test]
    fn test_new_plus_reassign_is_convergent() {
        // Wrapper type: `new_with_field_name` leaves the wrapper id random and
        // only the inner map deterministic; `reassign` canonicalises the wrapper
        // too. The CIP-I9 property is convergence — two independent
        // `new() + reassign("f")` mint the same id (stronger than matching
        // `new_with_field_name`). Inner-map determinism is covered by the
        // `UnorderedMap` tests.
        crate::env::reset_for_testing();
        let mut a: AuthoredMap<String, u32> = AuthoredMap::new();
        a.reassign_deterministic_id("items");
        let mut b: AuthoredMap<String, u32> = AuthoredMap::new();
        b.reassign_deterministic_id("items");
        assert_eq!(
            <AuthoredMap<String, u32> as crate::entities::Data>::id(&a),
            <AuthoredMap<String, u32> as crate::entities::Data>::id(&b),
        );
    }

    /// The tests name the OWNER, and an owner is an account.
    fn acct(bytes: [u8; 32]) -> AccountId {
        AccountId::from(bytes)
    }

    /// Who owns each entry, in key order.
    fn owners(map: &AuthoredMap<String, u64>) -> Vec<AccountId> {
        let mut entries = map.entries_with_owners().unwrap();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        entries.into_iter().map(|(owner, _, _)| owner).collect()
    }

    #[test]
    #[serial]
    fn insert_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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
    fn update_by_owner_succeeds() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();
        map.update(&"apple".to_owned(), 42).expect("owner update");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(42));
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );
    }

    #[test]
    #[serial]
    fn an_update_by_another_account_never_reaches_the_owner_s_entry() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();

        // Bob holds no "apple" of his own, so there is nothing for him to update.
        env::set_account_id(BOB);
        let err = map
            .update(&"apple".to_owned(), 99)
            .expect_err("bob holds no apple");
        assert!(
            err.to_string().to_lowercase().contains("not found"),
            "error should say the caller's entry is missing, got: {err}"
        );
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), None);
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), None);
        assert_eq!(
            map.get_by(&acct(ALICE), &"apple".to_owned()).unwrap(),
            Some(1)
        );
    }

    #[test]
    #[serial]
    fn update_missing_key_errors() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        let err = map
            .update(&"ghost".to_owned(), 1)
            .expect_err("missing key update must fail");
        assert!(
            err.to_string().to_lowercase().contains("not found")
                || err.to_string().to_lowercase().contains("record"),
            "error should indicate missing key, got: {err}"
        );
    }

    #[test]
    #[serial]
    fn remove_by_owner_succeeds() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();

        let removed = map.remove(&"apple".to_owned()).unwrap();
        assert_eq!(removed, Some(1));
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), None);
        assert_eq!(map.len().unwrap(), 0);
    }

    #[test]
    #[serial]
    fn a_remove_by_another_account_never_reaches_the_owner_s_entry() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_account_id(BOB);
        assert_eq!(map.remove(&"apple".to_owned()).unwrap(), None);
        assert_eq!(
            map.get_by(&acct(ALICE), &"apple".to_owned()).unwrap(),
            Some(1)
        );
        assert_eq!(map.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn remove_missing_key_is_none() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        assert_eq!(map.remove(&"ghost".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn each_account_writes_its_own_namespace() {
        env::reset_for_testing();

        let mut map = Root::new(AuthoredMap::<String, u64>::new);

        env::set_account_id(ALICE);
        map.insert("shared".to_owned(), 1).unwrap();

        // Bob's "shared" is his own entry, not a claim on Alice's.
        env::set_account_id(BOB);
        map.insert("shared".to_owned(), 2).unwrap();
        assert!(map.insert("shared".to_owned(), 3).is_err(), "bob holds it");
        map.update(&"shared".to_owned(), 20).unwrap();

        assert_eq!(map.len().unwrap(), 2);
        assert_eq!(map.get(&"shared".to_owned()).unwrap(), Some(20));
        assert_eq!(map.owner_of(&"shared".to_owned()).unwrap(), Some(acct(BOB)));
        assert_eq!(
            map.get_by(&acct(ALICE), &"shared".to_owned()).unwrap(),
            Some(1)
        );
        let mut everyone = map.entries_with_owners().unwrap();
        everyone.sort();
        assert_eq!(
            everyone,
            vec![
                (acct(ALICE), "shared".to_owned(), 1),
                (acct(BOB), "shared".to_owned(), 20)
            ]
        );

        // Alice still sees her original value.
        env::set_account_id(ALICE);
        assert_eq!(map.get(&"shared".to_owned()).unwrap(), Some(1));
        assert_eq!(map.my_entries().unwrap(), vec![("shared".to_owned(), 1)]);
    }

    /// **Two devices of one account share ownership of its entries.**
    ///
    /// The owner stamp moved onto accounts along with every other gate, so a
    /// laptop's entry and a phone's entry both resolve to the same owning
    /// account and either device may edit either one.
    ///
    /// The sharp assertion is the last one: a *different* account is still
    /// refused, which is what tells an account-keyed stamp apart from "any
    /// device may edit" — the latter would pass every assertion before it too.
    #[test]
    #[serial]
    fn two_devices_of_one_account_share_ownership_of_its_entries() {
        // One account; two of its devices. The account is unlike either device id
        // on purpose — where they matched, this could not tell an account-keyed
        // stamp from a device-keyed one.
        //
        // This assertion used to run the other way: the phone was refused the
        // laptop's entry, on the grounds that an account-keyed stamp would "let
        // two devices clobber each other's per-writer state". That conflated two
        // things. Per-writer state — an LWW register's tiebreak, a counter's
        // slot, an HLC seed — is keyed on `device_id` and still is; nothing here
        // moved it. The owner stamp is not per-writer state, it is an
        // access-control principal, and refusing a person their own data on a
        // second machine was the bug, not the protection.
        const ACCOUNT: [u8; 32] = [0xAC; 32];
        const LAPTOP: [u8; 32] = [0xD1; 32];
        const PHONE: [u8; 32] = [0xD2; 32];

        env::reset_for_testing();
        env::set_account_id(ACCOUNT);

        let mut notes = Root::new(AuthoredMap::<String, u64>::new);

        env::set_device_id(LAPTOP);
        notes.insert("from-laptop".to_owned(), 1).unwrap();
        env::set_device_id(PHONE);
        notes.insert("from-phone".to_owned(), 2).unwrap();

        assert_eq!(notes.len().unwrap(), 2, "neither device's entry was lost");
        // The ACCOUNT owns both — not the machine that happened to type them.
        assert_eq!(
            notes.owner_of(&"from-laptop".to_owned()).unwrap(),
            Some(acct(ACCOUNT))
        );
        assert_eq!(
            notes.owner_of(&"from-phone".to_owned()).unwrap(),
            Some(acct(ACCOUNT))
        );

        // Still the phone, and that is the point: one person, either machine.
        notes
            .update(&"from-laptop".to_owned(), 99)
            .expect("a second device of the owning account may edit its entry");
        assert_eq!(notes.get(&"from-laptop".to_owned()).unwrap(), Some(99));

        // The guard: "any device may edit" would pass everything above.
        env::set_account_id([0xEE; 32]);
        assert!(
            notes.update(&"from-laptop".to_owned(), 1234).is_err(),
            "a different account must still be refused someone else's entry"
        );
        assert_eq!(
            notes
                .get_by(&acct(ACCOUNT), &"from-laptop".to_owned())
                .unwrap(),
            Some(99)
        );
    }

    #[test]
    #[serial]
    fn owner_of_missing_key_is_none() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let map = Root::new(AuthoredMap::<String, u64>::new);
        assert_eq!(map.owner_of(&"ghost".to_owned()).unwrap(), None);
    }

    // Reproduction for the migrate_my_entries convert path: an owner re-write
    // via the REAL `update()` (the call migrate_my_entries makes) must PERSIST
    // the schema re-stamp. The existing owner_driven_convert tests hand a
    // bumped nonce straight to save_raw and never exercise update(); this pins
    // that the guard write actually advances the nonce enough for save_internal
    // to persist, otherwise migrate_my_entries reports `converted` while the
    // entry silently stays stale (no re-stamp, no node-log).
    #[test]
    #[serial]
    fn owner_update_persists_schema_restamp() {
        use calimero_sdk::event::NoEvent;
        use calimero_sdk::state::{AppState, AppStateInit};

        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct V2;
        impl AppStateInit for V2 {
            type Return = V2;
        }
        impl AppState for V2 {
            type Event<'a> = NoEvent;
            const SCHEMA_VERSION: u32 = 2;
        }
        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct Unversioned;
        impl AppStateInit for Unversioned {
            type Return = Unversioned;
        }
        impl AppState for Unversioned {
            type Event<'a> = NoEvent;
        }

        env::reset_for_testing();
        env::set_account_id(ALICE);

        // Insert at the default (unversioned 0) target — the "v1" stamp.
        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("k".to_owned(), 1).unwrap();
        assert_eq!(map.entry_schema_version(&"k".to_owned()).unwrap(), Some(0));

        // The binary is now v2.
        calimero_sdk::app::register_schema_version::<V2>();

        // The one-tap convert: owner re-writes through the SAME update() path
        // migrate_my_entries uses (value unchanged).
        map.update(&"k".to_owned(), 1).unwrap();

        // The re-stamp MUST have persisted.
        let after = map.entry_schema_version(&"k".to_owned()).unwrap();
        calimero_sdk::app::register_schema_version::<Unversioned>(); // reset global
        assert_eq!(
            after,
            Some(2),
            "owner update() must persist the schema re-stamp; got {after:?}"
        );
    }

    // Same as above but with the EXACT value type + read-then-write pattern the
    // scenario-32 fixture (and migrate_my_entries) use: AuthoredMap<_, LwwRegister>
    // re-written with the value read back from `get()`. An LwwRegister carries its
    // own HLC; if the entry nonce is taken from the (stale) register instead of a
    // fresh write nonce, save_internal's LWW gate drops the convert and the
    // re-stamp never persists.
    #[test]
    #[serial]
    fn owner_update_persists_schema_restamp_lww_readback() {
        use calimero_sdk::event::NoEvent;
        use calimero_sdk::state::{AppState, AppStateInit};

        use crate::collections::LwwRegister;

        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct V2;
        impl AppStateInit for V2 {
            type Return = V2;
        }
        impl AppState for V2 {
            type Event<'a> = NoEvent;
            const SCHEMA_VERSION: u32 = 2;
        }
        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct Unversioned;
        impl AppStateInit for Unversioned {
            type Return = Unversioned;
        }
        impl AppState for Unversioned {
            type Event<'a> = NoEvent;
        }

        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, LwwRegister<String>>::new);
        map.insert("k".to_owned(), LwwRegister::new("v1".to_owned()))
            .unwrap();
        assert_eq!(map.entry_schema_version(&"k".to_owned()).unwrap(), Some(0));

        calimero_sdk::app::register_schema_version::<V2>();

        // Mirror migrate_my_entries exactly: read the value, then write it back.
        let v = map.get(&"k".to_owned()).unwrap().expect("entry present");
        map.update(&"k".to_owned(), v).unwrap();

        let after = map.entry_schema_version(&"k".to_owned()).unwrap();
        calimero_sdk::app::register_schema_version::<Unversioned>();
        assert_eq!(
            after,
            Some(2),
            "owner update() of an LwwRegister read-back must persist the re-stamp; got {after:?}"
        );
    }

    #[test]
    #[serial]
    fn entry_schema_version_and_ownership_reflect_stored_metadata() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();

        // An owner write stamps the binary's current target schema version
        // (0 in the unit env, where no app is registered).
        assert_eq!(
            map.entry_schema_version(&"apple".to_owned()).unwrap(),
            Some(calimero_sdk::app::schema_version()),
        );
        assert!(map.owned_by_me(&"apple".to_owned()).unwrap());

        // A different executor does not hold it, and reads Alice's version by
        // naming her.
        env::set_account_id(BOB);
        assert!(!map.owned_by_me(&"apple".to_owned()).unwrap());
        assert_eq!(
            map.entry_schema_version_by(&acct(ALICE), &"apple".to_owned())
                .unwrap(),
            Some(calimero_sdk::app::schema_version()),
        );

        // Absent key: no version, not owned.
        env::set_account_id(ALICE);
        assert_eq!(map.entry_schema_version(&"ghost".to_owned()).unwrap(), None);
        assert!(!map.owned_by_me(&"ghost".to_owned()).unwrap());
    }

    #[test]
    #[serial]
    fn entries_contains_all_inserted_pairs() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("a".to_owned(), 1).unwrap();
        map.insert("b".to_owned(), 2).unwrap();
        env::set_account_id(BOB);
        map.insert("c".to_owned(), 3).unwrap();

        let pairs: Vec<_> = map.entries().unwrap().collect();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.contains(&("a".to_owned(), 1)));
        assert!(pairs.contains(&("b".to_owned(), 2)));
        assert!(pairs.contains(&("c".to_owned(), 3)));
    }

    /// `AuthoredMap` is now matched by the `#[app::state]` macro's
    /// `is_collection_type`, so `__assign_deterministic_ids()` calls
    /// `reassign_deterministic_id` on it. This must NOT strip owner stamps or
    /// drop entries: the inner map is built with a deterministic id, so its
    /// `reassign` is an idempotent no-op (no clear+reinsert) and only the outer
    /// wrapper id is canonicalised. This guards the macro change as non-breaking
    /// for existing maps carried through a migration.
    #[test]
    #[serial]
    fn reassign_deterministic_id_preserves_entries_and_owners() {
        env::reset_for_testing();

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new_with_field_name("entries"));
        env::set_account_id(ALICE);
        map.insert("apple".to_owned(), 1).expect("alice insert");
        env::set_account_id(BOB);
        map.insert("banana".to_owned(), 2).expect("bob insert");

        // Simulate the macro-driven id canonicalisation.
        map.reassign_deterministic_id("entries");

        // Values survive.
        assert_eq!(
            map.get_by(&acct(ALICE), &"apple".to_owned()).unwrap(),
            Some(1)
        );
        assert_eq!(map.get(&"banana".to_owned()).unwrap(), Some(2));
        assert_eq!(map.len().unwrap(), 2);
        // Owner stamps survive (not re-stamped to the calling executor).
        assert_eq!(owners(&map), [acct(ALICE), acct(BOB)]);

        // Idempotent: a second reassign is a no-op and still preserves everything.
        map.reassign_deterministic_id("entries");
        assert_eq!(owners(&map), [acct(ALICE), acct(BOB)]);
        assert_eq!(map.len().unwrap(), 2);
    }

    /// Non-vacuous counterpart: building with `new()` (random inner id) forces
    /// the reassign down the clear+reinsert path — the one that used to drop
    /// per-entry `StorageType` and downgrade authored entries to `Public`. The
    /// owner stamps must survive that path.
    #[test]
    #[serial]
    fn reassign_clear_reinsert_path_preserves_owner_stamps() {
        env::reset_for_testing();

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        env::set_account_id(ALICE);
        map.insert("apple".to_owned(), 1).expect("alice insert");
        env::set_account_id(BOB);
        map.insert("banana".to_owned(), 2).expect("bob insert");

        // Random inner id != deterministic id => real clear+reinsert (not the
        // no-op fast path the sibling test exercises).
        map.reassign_deterministic_id("entries");

        assert_eq!(
            map.get_by(&acct(ALICE), &"apple".to_owned()).unwrap(),
            Some(1)
        );
        assert_eq!(map.get(&"banana".to_owned()).unwrap(), Some(2));
        assert_eq!(map.len().unwrap(), 2);
        assert_eq!(owners(&map), [acct(ALICE), acct(BOB)]);
    }
}
