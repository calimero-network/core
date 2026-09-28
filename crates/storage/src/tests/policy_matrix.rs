//! Every write policy over every collection it can guard.
//!
//! `Guarded<C, P>` separates how entries are read (`C`: `UnorderedMap`,
//! `SortedMap`, `IndexedMap`) from who may change them (`P`). The rule a policy
//! enforces must not depend on the collection, so each policy runs the same
//! tests over all three: the stamp and rules an insert writes, what the owner,
//! a stranger and a moderator may do through the API, what a peer's signed
//! action is refused on apply, and that the collection's own reads (a scan, a
//! key range, an index query) see exactly what the policy admits.
//!
//! Keys are per owner under every owning policy: a stranger writing a key the
//! owner holds writes an entry of its own, and never reaches the owner's.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::collections::{
    Authored, ContentAddressed, IndexValue, Indexed, IndexedMap, LwwRegister, Moderated,
    ModeratedOnce, Root, SortedMap, UnorderedMap, WriteOnce,
};
use crate::entities::{Data, EntryRules, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::MainStorage;
use crate::tests::common::account_of_key;
use crate::tests::owned_rules::{
    act_as, apply, delete, is_gone, key, later, refused, rules_of, signed, update,
};

/// A note, indexed by tag so an `IndexedMap` can hold it.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
pub(crate) struct Note {
    tag: String,
    text: String,
}

impl Indexed for Note {
    const INDEXES: &'static [&'static str] = &["tag"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        if index == 0 {
            self.tag.encode_index(out);
        }
    }
}

fn note(tag: &str, text: &str) -> Note {
    Note {
        tag: tag.to_owned(),
        text: text.to_owned(),
    }
}

/// The founder, first moderator of a moderated collection.
fn founder() -> SigningKey {
    key(0x40)
}
fn alice() -> SigningKey {
    key(0xA1)
}
fn mallory() -> SigningKey {
    key(0xEE)
}

fn owner_of_stamp(id: Id) -> AccountId {
    match <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("present")
        .storage_type
    {
        StorageType::User { owner, .. } => owner,
        other => panic!("expected an owned entry, got {other:?}"),
    }
}

/// The collection's own way of listing what it holds: a scan, a key range or
/// an index query. Each must see exactly what the policy admits.
pub(crate) trait Listing {
    fn listed(&self) -> Vec<String>;
}

impl Listing for UnorderedMap<String, Note> {
    fn listed(&self) -> Vec<String> {
        let mut texts: Vec<_> = self
            .entries()
            .expect("entries")
            .map(|(_, n)| n.text)
            .collect();
        texts.sort();
        texts
    }
}

impl Listing for SortedMap<String, Note> {
    fn listed(&self) -> Vec<String> {
        self.prefix(b"n")
            .expect("prefix")
            .map(|(_, n)| n.text)
            .collect()
    }
}

impl Listing for IndexedMap<String, Note> {
    fn listed(&self) -> Vec<String> {
        let mut texts: Vec<_> = self
            .query("tag")
            .eq("t")
            .entries()
            .expect("query")
            .into_iter()
            .map(|(_, n)| n.text)
            .collect();
        texts.sort();
        texts
    }
}

/// Tests every owning policy shares: `Authored`, `WriteOnce`, `Moderated`,
/// `ModeratedOnce`, over the collection `$map`.
macro_rules! owning_tests {
    ($map:ty, immutable: $immutable:expr, moderated: $moderated:expr) => {
        type Map = $map;

        /// A collection the founder created, holding one note by Alice.
        fn setup() -> (Root<Map>, Id) {
            env::reset_for_testing();
            let _ = act_as(&founder());
            let mut map = Root::new(Map::new);
            let _ = act_as(&alice());
            map.insert("n1".to_owned(), note("t", "one"))
                .expect("insert");
            let id = map.entry_id(&"n1".to_owned());
            (map, id)
        }

        #[test]
        #[serial]
        fn an_insert_is_owned_by_the_inserter_under_the_policy_s_rules() {
            let (map, id) = setup();
            assert_eq!(owner_of_stamp(id), account_of_key(&alice()));
            let rules = rules_of(id);
            assert_eq!(rules.immutable, $immutable);
            assert_eq!(rules.moderators.is_some(), $moderated);
            assert_eq!(
                map.owner_of(&"n1".to_owned()).expect("owner"),
                Some(account_of_key(&alice()))
            );
            assert!(map.owned_by_me(&"n1".to_owned()).expect("mine"));
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "one"))
            );
        }

        #[test]
        #[serial]
        fn another_account_writing_a_held_key_gets_an_entry_of_its_own() {
            let (mut map, id) = setup();
            let _ = act_as(&mallory());
            assert!(!map.owned_by_me(&"n1".to_owned()).expect("mine"));
            assert_eq!(map.get(&"n1".to_owned()).expect("get"), None);
            map.insert("n1".to_owned(), note("t", "mine"))
                .expect("mallory's own n1");
            assert!(refused(map.insert("n1".to_owned(), note("t", "again"))));
            assert!(map.owned_by_me(&"n1".to_owned()).expect("mine"));
            assert_ne!(map.entry_id(&"n1".to_owned()), id);

            // Each reads its own; either reads the other's by name.
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "mine"))
            );
            assert_eq!(
                map.get_by(&account_of_key(&alice()), &"n1".to_owned())
                    .expect("get_by"),
                Some(note("t", "one"))
            );
            assert!(map
                .contains_by(&account_of_key(&alice()), &"n1".to_owned())
                .expect("contains_by"));
            let _ = act_as(&alice());
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "one"))
            );

            // The collection's reads show both, and count both. A sorted map
            // orders one key's entries by id, so compare them as a set.
            let mut listed = map.listed();
            listed.sort();
            assert_eq!(listed, ["mine", "one"]);
            assert_eq!(map.len().expect("len"), 2);
            let mut holders: Vec<_> = map
                .entries_at(&"n1".to_owned())
                .expect("entries_at")
                .into_iter()
                .map(|(owner, _)| owner)
                .collect();
            holders.sort();
            let mut expected = vec![account_of_key(&alice()), account_of_key(&mallory())];
            expected.sort();
            assert_eq!(holders, expected);
        }

        #[test]
        #[serial]
        fn every_account_adds_its_own_entries_and_the_collection_lists_them() {
            let (mut map, _id) = setup();
            let _ = act_as(&mallory());
            map.insert("n2".to_owned(), note("t", "two"))
                .expect("insert");
            assert_eq!(
                map.owner_of(&"n2".to_owned()).expect("owner"),
                Some(account_of_key(&mallory()))
            );
            assert_eq!(map.listed(), ["one", "two"]);
            assert_eq!(map.len().expect("len"), 2);
        }

        #[test]
        #[serial]
        fn a_stranger_s_signed_delete_is_refused_on_apply() {
            let (map, id) = setup();
            let action = signed(
                delete(id, later()),
                account_of_key(&alice()),
                rules_of(id),
                &mallory(),
                later(),
            );
            assert!(apply(action, account_of_key(&mallory())).is_err());
            assert!(!is_gone(id));
            assert_eq!(map.listed(), ["one"]);
        }

        #[test]
        #[serial]
        fn a_stranger_s_signed_rewrite_is_refused_on_apply() {
            let (map, id) = setup();
            let action = signed(
                update(id, (*map).id(), b"forged".to_vec()),
                account_of_key(&alice()),
                rules_of(id),
                &mallory(),
                later(),
            );
            assert!(apply(action, account_of_key(&mallory())).is_err());
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "one"))
            );
        }

        #[test]
        #[serial]
        fn a_signed_write_that_changes_the_rules_is_refused_on_apply() {
            let (_map, id) = setup();
            let relaxed = EntryRules {
                immutable: !$immutable,
                moderators: rules_of(id).moderators,
            };
            let action = signed(
                delete(id, later()),
                account_of_key(&alice()),
                relaxed,
                &alice(),
                later(),
            );
            assert!(apply(action, account_of_key(&alice())).is_err());
            assert!(!is_gone(id));
        }
    };
}

/// Tests for the policies whose owner may edit: `Authored`, `Moderated`.
macro_rules! editable_tests {
    () => {
        #[test]
        #[serial]
        fn the_owner_updates_and_modifies_and_a_stranger_cannot() {
            let (mut map, _id) = setup();
            // Mallory holds no n1, so her edits have nothing to reach.
            let _ = act_as(&mallory());
            assert!(map.update(&"n1".to_owned(), note("t", "defaced")).is_err());
            assert!(map
                .modify(&"n1".to_owned(), |n| n.text = "defaced".to_owned())
                .is_err());

            let _ = act_as(&alice());
            map.update(&"n1".to_owned(), note("t", "uno"))
                .expect("update");
            assert_eq!(map.listed(), ["uno"]);
            let _ = map
                .modify(&"n1".to_owned(), |n| n.text = "eins".to_owned())
                .expect("modify");
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "eins"))
            );
            assert_eq!(map.listed(), ["eins"]);
        }

        #[test]
        #[serial]
        fn the_owner_removes_and_a_stranger_cannot() {
            let (mut map, id) = setup();
            let _ = act_as(&mallory());
            assert_eq!(map.remove(&"n1".to_owned()).expect("remove"), None);
            assert_eq!(map.listed(), ["one"]);

            let _ = act_as(&alice());
            assert_eq!(
                map.remove(&"n1".to_owned()).expect("remove"),
                Some(note("t", "one"))
            );
            assert!(map.listed().is_empty());
            assert!(is_gone(id));
        }

        #[test]
        #[serial]
        fn every_node_accepts_the_owner_s_signed_delete() {
            let (map, id) = setup();
            let action = signed(
                delete(id, later()),
                account_of_key(&alice()),
                rules_of(id),
                &alice(),
                later(),
            );
            apply(action, account_of_key(&alice())).expect("the owner deletes");
            assert!(is_gone(id));
            assert!(map.listed().is_empty());
        }
    };
}

/// Tests for the immutable policies: `WriteOnce`, `ModeratedOnce`.
macro_rules! immutable_tests {
    () => {
        #[test]
        #[serial]
        fn no_node_accepts_the_owner_s_signed_rewrite() {
            let (map, id) = setup();
            let action = signed(
                update(id, (*map).id(), b"rewritten".to_vec()),
                account_of_key(&alice()),
                rules_of(id),
                &alice(),
                later(),
            );
            assert!(matches!(
                apply(action, account_of_key(&alice())),
                Err(StorageError::ActionNotAllowed(_))
            ));
            assert_eq!(
                map.get(&"n1".to_owned()).expect("get"),
                Some(note("t", "one"))
            );
        }

        #[test]
        #[serial]
        fn the_owner_cannot_take_its_own_key_again() {
            let (mut map, _id) = setup();
            assert!(refused(map.insert("n1".to_owned(), note("t", "again"))));
            assert_eq!(map.listed(), ["one"]);
        }
    };
}

/// Tests for `WriteOnce`: nobody deletes, the owner included.
macro_rules! write_once_tests {
    () => {
        #[test]
        #[serial]
        fn no_node_accepts_the_owner_s_signed_delete() {
            let (map, id) = setup();
            let action = signed(
                delete(id, later()),
                account_of_key(&alice()),
                rules_of(id),
                &alice(),
                later(),
            );
            assert!(apply(action, account_of_key(&alice())).is_err());
            assert!(!is_gone(id));
            assert_eq!(map.listed(), ["one"]);
        }
    };
}

/// Tests for the moderated policies: `Moderated`, `ModeratedOnce`.
macro_rules! moderated_tests {
    (owner_may_delete: $owner_may_delete:expr) => {
        #[test]
        #[serial]
        fn the_founder_is_the_first_moderator() {
            let (map, _id) = setup();
            assert_eq!(
                map.moderators(),
                [account_of_key(&founder())].into_iter().collect()
            );
            assert!(map.is_moderator(&account_of_key(&founder())));
            assert!(!map.is_moderator(&account_of_key(&alice())));
        }

        #[test]
        #[serial]
        fn a_moderator_removes_someone_else_s_entry() {
            let (mut map, id) = setup();
            let _ = act_as(&founder());
            assert_eq!(
                map.remove(&"n1".to_owned()).expect("the founder's own"),
                None
            );
            assert_eq!(
                map.remove_by(&account_of_key(&alice()), &"n1".to_owned())
                    .expect("moderate"),
                Some(note("t", "one"))
            );
            assert!(is_gone(id));
            assert!(map.listed().is_empty());
        }

        #[test]
        #[serial]
        fn every_node_accepts_a_moderator_s_signed_delete() {
            let (map, id) = setup();
            let action = signed(
                delete(id, later()),
                account_of_key(&alice()),
                rules_of(id),
                &founder(),
                later(),
            );
            apply(action, account_of_key(&founder())).expect("a moderator deletes");
            assert!(is_gone(id));
            assert!(map.listed().is_empty());
        }

        #[test]
        #[serial]
        fn a_stranger_can_neither_remove_nor_appoint_moderators() {
            let (mut map, _id) = setup();
            let _ = act_as(&mallory());
            assert!(refused(
                map.remove_by(&account_of_key(&alice()), &"n1".to_owned())
            ));
            assert!(map
                .set_moderators([account_of_key(&mallory())].into_iter().collect())
                .is_err());
            assert!(!map.is_moderator(&account_of_key(&mallory())));
        }

        #[test]
        #[serial]
        fn an_appointed_moderator_removes_and_a_revoked_one_cannot() {
            let (mut map, _id) = setup();
            let _ = act_as(&founder());
            map.set_moderators(
                [account_of_key(&founder()), account_of_key(&mallory())]
                    .into_iter()
                    .collect(),
            )
            .expect("appoint");
            assert!(map.is_moderator(&account_of_key(&mallory())));

            let _ = act_as(&alice());
            map.insert("n2".to_owned(), note("t", "two"))
                .expect("insert");
            let _ = act_as(&mallory());
            assert_eq!(
                map.remove_by(&account_of_key(&alice()), &"n2".to_owned())
                    .expect("an appointed moderator"),
                Some(note("t", "two"))
            );

            let _ = act_as(&founder());
            map.set_moderators([account_of_key(&founder())].into_iter().collect())
                .expect("revoke");
            let _ = act_as(&mallory());
            assert!(refused(
                map.remove_by(&account_of_key(&alice()), &"n1".to_owned())
            ));
            assert_eq!(map.listed(), ["one"]);
            crate::tests::common::assert_every_shared_entity_is_bound();
        }

        #[test]
        #[serial]
        fn the_owner_s_own_removal_follows_the_policy() {
            let (mut map, id) = setup();
            let removed = map.remove(&"n1".to_owned());
            if $owner_may_delete {
                assert_eq!(removed.expect("the owner removes"), Some(note("t", "one")));
                assert!(is_gone(id));
            } else {
                assert!(refused(removed));
                assert_eq!(map.listed(), ["one"]);
            }
        }
    };
}

mod authored {
    use super::*;

    mod unordered_map {
        use super::*;
        owning_tests!(Authored<UnorderedMap<String, Note>>, immutable: false, moderated: false);
        editable_tests!();
    }
    mod sorted_map {
        use super::*;
        owning_tests!(Authored<SortedMap<String, Note>>, immutable: false, moderated: false);
        editable_tests!();
    }
    mod indexed_map {
        use super::*;
        owning_tests!(Authored<IndexedMap<String, Note>>, immutable: false, moderated: false);
        editable_tests!();
    }
}

mod write_once {
    use super::*;

    mod unordered_map {
        use super::*;
        owning_tests!(WriteOnce<UnorderedMap<String, Note>>, immutable: true, moderated: false);
        immutable_tests!();
        write_once_tests!();
    }
    mod sorted_map {
        use super::*;
        owning_tests!(WriteOnce<SortedMap<String, Note>>, immutable: true, moderated: false);
        immutable_tests!();
        write_once_tests!();
    }
    mod indexed_map {
        use super::*;
        owning_tests!(WriteOnce<IndexedMap<String, Note>>, immutable: true, moderated: false);
        immutable_tests!();
        write_once_tests!();
    }
}

mod moderated {
    use super::*;

    mod unordered_map {
        use super::*;
        owning_tests!(Moderated<UnorderedMap<String, Note>>, immutable: false, moderated: true);
        editable_tests!();
        moderated_tests!(owner_may_delete: true);
    }
    mod sorted_map {
        use super::*;
        owning_tests!(Moderated<SortedMap<String, Note>>, immutable: false, moderated: true);
        editable_tests!();
        moderated_tests!(owner_may_delete: true);
    }
    mod indexed_map {
        use super::*;
        owning_tests!(Moderated<IndexedMap<String, Note>>, immutable: false, moderated: true);
        editable_tests!();
        moderated_tests!(owner_may_delete: true);
    }
}

mod moderated_once {
    use super::*;

    mod unordered_map {
        use super::*;
        owning_tests!(ModeratedOnce<UnorderedMap<String, Note>>, immutable: true, moderated: true);
        immutable_tests!();
        moderated_tests!(owner_may_delete: false);
    }
    mod sorted_map {
        use super::*;
        owning_tests!(ModeratedOnce<SortedMap<String, Note>>, immutable: true, moderated: true);
        immutable_tests!();
        moderated_tests!(owner_may_delete: false);
    }
    mod indexed_map {
        use super::*;
        owning_tests!(ModeratedOnce<IndexedMap<String, Note>>, immutable: true, moderated: true);
        immutable_tests!();
        moderated_tests!(owner_may_delete: false);
    }
}

// ---------------------------------------------------------------------------
// Content-addressed
// ---------------------------------------------------------------------------

/// `Listing` for a content-addressed collection, keyed by hash.
trait HashListing {
    fn listed(&self) -> Vec<String>;
}

impl HashListing for UnorderedMap<[u8; 32], Note> {
    fn listed(&self) -> Vec<String> {
        let mut texts: Vec<_> = self
            .entries()
            .expect("entries")
            .map(|(_, n)| n.text)
            .collect();
        texts.sort();
        texts
    }
}

impl HashListing for SortedMap<[u8; 32], Note> {
    fn listed(&self) -> Vec<String> {
        let mut texts: Vec<_> = self
            .entries()
            .expect("entries")
            .map(|(_, n)| n.text)
            .collect();
        texts.sort();
        texts
    }
}

impl HashListing for IndexedMap<[u8; 32], Note> {
    fn listed(&self) -> Vec<String> {
        let mut texts: Vec<_> = self
            .query("tag")
            .eq("t")
            .entries()
            .expect("query")
            .into_iter()
            .map(|(_, n)| n.text)
            .collect();
        texts.sort();
        texts
    }
}

macro_rules! content_addressed_tests {
    ($map:ty) => {
        type Map = $map;

        fn setup() -> (Root<Map>, [u8; 32]) {
            env::reset_for_testing();
            let _ = act_as(&alice());
            let mut map = Root::new(Map::new);
            let hash = map.insert(note("t", "one")).expect("insert");
            (map, hash)
        }

        #[test]
        #[serial]
        fn an_entry_is_keyed_by_the_hash_of_its_bytes_and_stamped_frozen() {
            let (map, hash) = setup();
            let expected: [u8; 32] =
                Sha256::digest(borsh::to_vec(&note("t", "one")).expect("bytes")).into();
            assert_eq!(hash, expected);
            assert_eq!(map.get(&hash).expect("get"), Some(note("t", "one")));
            let id = map.entry_id(&hash);
            let metadata = <Index<MainStorage>>::get_metadata(id)
                .expect("metadata")
                .expect("present");
            assert_eq!(metadata.storage_type, StorageType::Frozen);
            assert_eq!(
                metadata.crdt_type,
                Some(crate::collections::crdt_meta::CrdtType::FrozenStorage),
                "the tag a host-side repair stores it back as frozen by"
            );
        }

        #[test]
        #[serial]
        fn equal_content_from_anyone_is_stored_once() {
            let (mut map, hash) = setup();
            let _ = act_as(&mallory());
            assert_eq!(map.insert(note("t", "one")).expect("insert"), hash);
            let other = map.insert(note("t", "two")).expect("insert");
            assert_ne!(other, hash);
            assert_eq!(map.len().expect("len"), 2);
            assert_eq!(map.listed(), ["one", "two"]);
        }

        #[test]
        #[serial]
        fn no_node_accepts_a_delete() {
            let (map, hash) = setup();
            let id = map.entry_id(&hash);
            let action = crate::action::Action::DeleteRef {
                id,
                deleted_at: later(),
                metadata: Metadata {
                    storage_type: StorageType::Frozen,
                    ..Metadata::default()
                },
            };
            assert!(apply(action, account_of_key(&alice())).is_err());
            assert!(!is_gone(id));
            assert_eq!(map.listed(), ["one"]);
        }

        #[test]
        #[serial]
        fn no_node_accepts_a_rewrite() {
            let (map, hash) = setup();
            let id = map.entry_id(&hash);
            let action = crate::action::Action::Update {
                id,
                data: b"rewritten".to_vec(),
                ancestors: vec![crate::entities::ChildInfo::new(
                    (*map).id(),
                    [0; 32],
                    Metadata::default(),
                )],
                metadata: Metadata {
                    updated_at: later().into(),
                    storage_type: StorageType::Frozen,
                    ..Metadata::default()
                },
            };
            assert!(apply(action, account_of_key(&alice())).is_err());
            assert_eq!(map.get(&hash).expect("get"), Some(note("t", "one")));
        }
    };
}

mod content_addressed {
    use super::*;

    mod unordered_map {
        use super::*;
        content_addressed_tests!(ContentAddressed<UnorderedMap<[u8; 32], Note>>);
    }
    mod sorted_map {
        use super::*;
        content_addressed_tests!(ContentAddressed<SortedMap<[u8; 32], Note>>);
    }
    mod indexed_map {
        use super::*;
        content_addressed_tests!(ContentAddressed<IndexedMap<[u8; 32], Note>>);
    }
}

// ---------------------------------------------------------------------------
// Collections nested inside each policy's entries
// ---------------------------------------------------------------------------

type Revisions = UnorderedMap<String, LwwRegister<u64>>;

fn revisions_with(key: &str, value: u64) -> Revisions {
    let mut revisions = Revisions::new();
    let _ = revisions
        .insert(key.to_owned(), LwwRegister::new(value))
        .expect("insert");
    revisions
}

/// An editable entry's nested collection belongs to the entry's owner.
macro_rules! owned_nested_tests {
    ($map:ty) => {
        type Map = $map;

        /// Alice's entry, holding a nested collection with one revision.
        fn setup() -> Root<Map> {
            env::reset_for_testing();
            let _ = act_as(&founder());
            let mut map = Root::new(Map::new);
            let _ = act_as(&alice());
            map.insert("n1".to_owned(), revisions_with("r1", 1))
                .expect("insert");
            map
        }

        /// Alice's collection, read by whoever is acting.
        fn nested(map: &Root<Map>) -> Revisions {
            map.get_by(&account_of_key(&alice()), &"n1".to_owned())
                .expect("get")
                .expect("n1")
        }

        #[test]
        #[serial]
        fn the_nested_entries_carry_the_owner_s_stamp() {
            let map = setup();
            let revisions = nested(&map);
            assert_eq!(revisions.len().expect("len"), 1);
            assert_eq!(
                owner_of_stamp(revisions.entry_id("r1")),
                account_of_key(&alice())
            );
        }

        #[test]
        #[serial]
        fn a_stranger_cannot_write_the_nested_collection() {
            let map = setup();
            let _ = act_as(&mallory());
            let mut revisions = nested(&map);
            assert!(refused(
                revisions.insert("r2".to_owned(), LwwRegister::new(2))
            ));
            assert!(refused(revisions.remove("r1")));
            assert_eq!(nested(&map).len().expect("len"), 1);
        }

        #[test]
        #[serial]
        fn the_owner_writes_the_nested_collection_under_its_own_stamp() {
            let map = setup();
            let mut revisions = nested(&map);
            let _ = revisions
                .insert("r2".to_owned(), LwwRegister::new(2))
                .expect("the owner adds a revision");
            let revisions = nested(&map);
            assert_eq!(revisions.len().expect("len"), 2);
            assert_eq!(
                owner_of_stamp(revisions.entry_id("r2")),
                account_of_key(&alice())
            );
            let mut revisions = nested(&map);
            let _ = revisions.remove("r1").expect("the owner removes one");
            assert_eq!(nested(&map).len().expect("len"), 1);
        }
    };
}

/// An immutable entry seals its nested collection: it can only be stored
/// empty, and nobody, the owner included, can ever write into it.
macro_rules! sealed_nested_tests {
    ($map:ty) => {
        type Map = $map;

        fn setup() -> Root<Map> {
            env::reset_for_testing();
            let _ = act_as(&founder());
            let mut map = Root::new(Map::new);
            let _ = act_as(&alice());
            map.insert("empty".to_owned(), Revisions::new())
                .expect("an empty nested collection is stored");
            map
        }

        /// Alice's collection, read by whoever is acting.
        fn nested(map: &Root<Map>) -> Revisions {
            map.get_by(&account_of_key(&alice()), &"empty".to_owned())
                .expect("get")
                .expect("empty")
        }

        #[test]
        #[serial]
        fn a_filled_nested_collection_cannot_be_stored() {
            let mut map = setup();
            assert!(refused(
                map.insert("full".to_owned(), revisions_with("r1", 1))
            ));
            assert!(map.get(&"full".to_owned()).expect("get").is_none());
        }

        #[test]
        #[serial]
        fn the_owner_cannot_write_into_it() {
            let map = setup();
            let mut revisions = nested(&map);
            assert!(refused(
                revisions.insert("r1".to_owned(), LwwRegister::new(1))
            ));
            assert!(nested(&map).is_empty().expect("empty"));
        }

        #[test]
        #[serial]
        fn a_stranger_cannot_write_into_it() {
            let map = setup();
            let _ = act_as(&mallory());
            let mut revisions = nested(&map);
            assert!(refused(
                revisions.insert("r1".to_owned(), LwwRegister::new(1))
            ));
            assert!(nested(&map).is_empty().expect("empty"));
        }
    };
}

mod nested {
    use super::*;

    mod authored_unordered {
        use super::*;
        owned_nested_tests!(Authored<UnorderedMap<String, Revisions>>);
    }
    mod authored_sorted {
        use super::*;
        owned_nested_tests!(Authored<SortedMap<String, Revisions>>);
    }
    mod moderated_unordered {
        use super::*;
        owned_nested_tests!(Moderated<UnorderedMap<String, Revisions>>);
    }
    mod moderated_sorted {
        use super::*;
        owned_nested_tests!(Moderated<SortedMap<String, Revisions>>);
    }
    mod write_once_unordered {
        use super::*;
        sealed_nested_tests!(WriteOnce<UnorderedMap<String, Revisions>>);
    }
    mod write_once_sorted {
        use super::*;
        sealed_nested_tests!(WriteOnce<SortedMap<String, Revisions>>);
    }
    mod moderated_once_unordered {
        use super::*;
        sealed_nested_tests!(ModeratedOnce<UnorderedMap<String, Revisions>>);
    }
    mod moderated_once_sorted {
        use super::*;
        sealed_nested_tests!(ModeratedOnce<SortedMap<String, Revisions>>);
    }
}
