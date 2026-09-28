//! A collection nested inside a guarded entry is guarded by it, at any depth.
//!
//! Each test builds state through the public collections, then plays a peer:
//! first an honest one writing through the API as someone without authority
//! (refused locally, except in a writer-set cell, whose writes every peer
//! checks against the cell), then a patched one that skips the API and hands `Interface::apply_action`
//! the exact bytes a forged write would carry. The first must be refused
//! locally. The second may be stored, since the stamp it claims is one the
//! receiver cannot tie to the enclosing entry, but must never be READ: every
//! node filters it identically, so it changes no answer anywhere.

use borsh::BorshSerialize;
use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{
    compute_id, owned_keyed_entry_id, Authored, AuthoredVector, ContentAddressed, IndexValue,
    Indexed, IndexedMap, LwwRegister, Root, SortedMap, UnorderedMap, UnorderedSet, UserStorage,
    Vector, WriterSetCell,
};
use crate::entities::{ChildInfo, Data, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface, StorageError};
use crate::store::MainStorage;
use crate::tests::common::{
    account_of_key, apply_ctx_for, build_signed_member_action, create_signed_user_add_action,
    create_test_owner, map_entry_bytes,
};

type MainInterface = Interface<MainStorage>;
type Tags = UnorderedMap<String, LwwRegister<u64>>;
type Posts = Authored<UnorderedMap<String, Tags>>;

const ALICE: [u8; 32] = [0xA1; 32];
const BOB: [u8; 32] = [0xB0; 32];

fn account(bytes: [u8; 32]) -> AccountId {
    AccountId::from(bytes)
}

fn act_as(bytes: [u8; 32]) {
    env::set_account_id(bytes);
}

fn reg(value: u64) -> LwwRegister<u64> {
    LwwRegister::new(value)
}

fn stamp_of(id: Id) -> StorageType {
    <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("present")
        .storage_type
}

fn owned_by(bytes: [u8; 32]) -> StorageType {
    StorageType::User {
        rules: crate::entities::EntryRules::OWNED,
        owner: account(bytes),
        signature_data: None,
    }
}

fn is_owned_by(stamp: &StorageType, bytes: [u8; 32]) -> bool {
    matches!(stamp, StorageType::User { owner, .. } if *owner == account(bytes))
}

fn refused<T: core::fmt::Debug>(result: Result<T, crate::collections::StoreError>) -> bool {
    matches!(
        result,
        Err(crate::collections::StoreError::StorageError(
            StorageError::ActionNotAllowed(_)
        ))
    )
}

/// A `Public` entry for `key` claiming `collection` as its parent, as a patched
/// peer would send it. `Public` carries no signature, so every node accepts it.
fn forge_public_entry<K, V>(collection: Id, key: &K, value: &V) -> Id
where
    K: BorshSerialize + AsRef<[u8]>,
    V: BorshSerialize,
{
    let (id, result) = try_public_entry(collection, key, value);
    result.expect("a public add applies");
    id
}

/// [`forge_public_entry`], returning whether the node took it.
fn try_public_entry<K, V>(
    collection: Id,
    key: &K,
    value: &V,
) -> (Id, Result<(), crate::interface::StorageError>)
where
    K: BorshSerialize + AsRef<[u8]>,
    V: BorshSerialize,
{
    let id = compute_id(collection, key.as_ref());
    let now = env::time_now();
    let metadata = Metadata {
        created_at: now,
        updated_at: now.into(),
        ..Metadata::default()
    };
    let action = Action::Add {
        id,
        data: map_entry_bytes(id, key, value),
        ancestors: vec![ChildInfo::new(collection, [0; 32], Metadata::default())],
        metadata,
    };
    (
        id,
        MainInterface::apply_action(action, &ApplyContext::empty()),
    )
}

/// A correctly signed entry, owned by the peer that signed it, claiming
/// `collection` as its parent. The signature is genuine, and the id is the
/// signer's own for `key`, as apply requires; the owner is wrong for the
/// collection.
fn forge_self_signed_entry<K, V>(collection: Id, key: &K, value: &V, signer: &SigningKey) -> Id
where
    K: BorshSerialize + AsRef<[u8]>,
    V: BorshSerialize,
{
    let owner = account_of_key(signer);
    let id = owned_keyed_entry_id(compute_id(collection, key.as_ref()), &owner);
    let mut action = create_signed_user_add_action(
        signer,
        owner,
        id,
        map_entry_bytes(id, key, value),
        env::time_now() + 1_000_000_000,
    );
    if let Action::Add { ancestors, .. } = &mut action {
        *ancestors = vec![ChildInfo::new(collection, [0; 32], Metadata::default())];
    }
    MainInterface::apply_action(action, &apply_ctx_for(owner))
        .expect("a correctly signed entry applies");
    id
}

/// Alice's post `p1`, whose tag map already holds `rust`.
fn alices_post() -> Root<Posts> {
    env::reset_for_testing();
    act_as(ALICE);
    let mut posts = Root::new(Posts::new);
    let mut tags = Tags::new();
    let _ = tags.insert("rust".to_owned(), reg(1)).expect("insert");
    posts.insert("p1".to_owned(), tags).expect("insert");
    posts
}

/// Alice's tags, read by whoever is acting.
fn tags_of(posts: &Posts) -> Tags {
    posts
        .get_by(&account(ALICE), &"p1".to_owned())
        .expect("get")
        .expect("p1")
}

fn tag_names(tags: &Tags) -> Vec<String> {
    let mut names: Vec<_> = tags.entries().expect("entries").map(|(k, _)| k).collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Stamping
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn entries_filled_in_before_the_insert_are_owned_by_the_author() {
    let posts = alices_post();
    let tags = tags_of(&posts);
    let rust = tags.entry_id("rust");
    assert_eq!(stamp_of(rust), owned_by(ALICE));
}

#[test]
#[serial]
fn entries_added_after_the_insert_are_owned_by_the_author() {
    let posts = alices_post();
    let mut tags = tags_of(&posts);
    let _ = tags.insert("sync".to_owned(), reg(2)).expect("owner adds");
    assert_eq!(stamp_of(tags.entry_id("sync")), owned_by(ALICE));

    // A fresh read sees it, still owned.
    let tags = tags_of(&posts);
    assert_eq!(tag_names(&tags), ["rust", "sync"]);
    assert_eq!(tags.len().expect("len"), 2);
}

#[test]
#[serial]
fn a_plain_map_of_maps_stays_open_to_everyone() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut boards = Root::new(UnorderedMap::<String, Tags>::new);
    let _ = boards
        .insert("dev".to_owned(), Tags::new())
        .expect("insert");

    act_as(BOB);
    let mut tags = boards.get("dev").expect("get").expect("dev").into_inner();
    let _ = tags
        .insert("bob".to_owned(), reg(1))
        .expect("anyone may write");
    assert!(matches!(
        stamp_of(tags.entry_id("bob")),
        StorageType::Public
    ));
    let tags = boards.get("dev").expect("get").expect("dev").into_inner();
    assert_eq!(tag_names(&tags), ["bob"]);
}

// ---------------------------------------------------------------------------
// Honest peers without authority are refused locally
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn another_account_cannot_add_update_or_remove_nested_entries() {
    let posts = alices_post();
    act_as(BOB);
    let mut tags = tags_of(&posts);

    assert!(refused(tags.insert("spam".to_owned(), reg(9))), "add");
    assert!(refused(tags.insert("rust".to_owned(), reg(9))), "replace");
    assert!(refused(tags.remove("rust")), "remove");
    assert!(refused(tags.clear()), "clear");
    assert!(refused(tags.get_mut("rust").map(|_| ())), "edit in place");

    act_as(ALICE);
    let tags = tags_of(&posts);
    assert_eq!(tag_names(&tags), ["rust"]);
    assert_eq!(*tags.get("rust").expect("get").expect("rust").get(), 1);
}

#[test]
#[serial]
fn the_owner_can_edit_and_remove_nested_entries() {
    let posts = alices_post();
    let mut tags = tags_of(&posts);
    let _ = tags.insert("rust".to_owned(), reg(5)).expect("replace");
    assert_eq!(*tags.get("rust").expect("get").expect("rust").get(), 5);
    let _ = tags.remove("rust").expect("remove");
    assert!(tag_names(&tags_of(&posts)).is_empty());
}

#[test]
#[serial]
fn protection_reaches_every_depth() {
    type Leaf = UnorderedMap<String, LwwRegister<u64>>;
    type Mid = UnorderedMap<String, Leaf>;
    type Deep = Authored<UnorderedMap<String, Mid>>;

    env::reset_for_testing();
    act_as(ALICE);
    let mut root = Root::new(Deep::new);
    let mut mid = Mid::new();
    let mut leaf = Leaf::new();
    let _ = leaf.insert("x".to_owned(), reg(1)).expect("insert");
    let _ = mid.insert("m".to_owned(), leaf).expect("insert");
    root.insert("top".to_owned(), mid).expect("insert");

    let mid = root.get(&"top".to_owned()).expect("get").expect("top");
    let mut leaf = mid.get("m").expect("get").expect("m").into_inner();
    assert_eq!(stamp_of(mid.entry_id("m")), owned_by(ALICE), "depth 2");
    assert_eq!(stamp_of(leaf.entry_id("x")), owned_by(ALICE), "depth 3");
    let _ = leaf.insert("y".to_owned(), reg(2)).expect("owner, depth 3");
    assert_eq!(stamp_of(leaf.entry_id("y")), owned_by(ALICE));

    act_as(BOB);
    let mut mid = root
        .get_by(&account(ALICE), &"top".to_owned())
        .expect("get")
        .expect("top");
    let mut leaf = mid.get("m").expect("get").expect("m").into_inner();
    assert!(refused(mid.insert("n".to_owned(), Leaf::new())), "depth 2");
    assert!(refused(leaf.insert("z".to_owned(), reg(3))), "depth 3");
    assert!(refused(leaf.remove("x")), "depth 3 remove");
}

#[test]
#[serial]
fn every_collection_kind_nested_in_an_owned_entry_is_owned() {
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct Item(u64);
    impl Indexed for Item {
        const INDEXES: &'static [&'static str] = &["n"];
        fn index_keys(&self, _index: usize, out: &mut Vec<Vec<u8>>) {
            self.0.encode_index(out);
        }
    }

    env::reset_for_testing();
    act_as(ALICE);

    // Vector.
    let mut lists = Root::new(Authored::<UnorderedMap<String, Vector<LwwRegister<u64>>>>::new);
    let mut list = Vector::new();
    list.push(reg(1)).expect("push");
    lists.insert("l".to_owned(), list).expect("insert");
    let mut list = lists.get(&"l".to_owned()).expect("get").expect("l");
    list.push(reg(2)).expect("owner pushes");
    assert_eq!(list.len().expect("len"), 2);

    // Set.
    let mut sets = Root::new(Authored::<UnorderedMap<String, UnorderedSet<String>>>::new);
    let mut set = UnorderedSet::new();
    let _ = set.insert("a".to_owned()).expect("insert");
    sets.insert("s".to_owned(), set).expect("insert");

    // Sorted map.
    let mut sorted =
        Root::new(Authored::<UnorderedMap<String, SortedMap<String, LwwRegister<u64>>>>::new);
    let mut ordered = SortedMap::new();
    let _ = ordered.insert("b".to_owned(), reg(1)).expect("insert");
    let _ = ordered.insert("a".to_owned(), reg(2)).expect("insert");
    sorted.insert("o".to_owned(), ordered).expect("insert");

    // Indexed map.
    let mut indexed = Root::new(Authored::<UnorderedMap<String, IndexedMap<String, Item>>>::new);
    let mut items = IndexedMap::new();
    let _ = items.insert("i".to_owned(), Item(7)).expect("insert");
    indexed.insert("x".to_owned(), items).expect("insert");

    act_as(BOB);
    let alice = account(ALICE);
    let mut list = lists
        .get_by(&alice, &"l".to_owned())
        .expect("get")
        .expect("l");
    assert!(refused(list.push(reg(9))), "vector push");
    assert!(refused(list.update(0, reg(9))), "vector update");
    let mut set = sets
        .get_by(&alice, &"s".to_owned())
        .expect("get")
        .expect("s");
    assert!(refused(set.insert("b".to_owned())), "set insert");
    assert!(refused(set.remove("a")), "set remove");
    let mut ordered = sorted
        .get_by(&alice, &"o".to_owned())
        .expect("get")
        .expect("o");
    assert!(
        refused(ordered.insert("c".to_owned(), reg(9))),
        "sorted insert"
    );
    let mut items = indexed
        .get_by(&alice, &"x".to_owned())
        .expect("get")
        .expect("x");
    assert!(
        refused(items.insert("j".to_owned(), Item(9))),
        "indexed insert"
    );
    assert!(refused(items.remove("i")), "indexed remove");

    act_as(ALICE);
    let ordered = sorted.get(&"o".to_owned()).expect("get").expect("o");
    let keys: Vec<_> = ordered.keys().expect("keys").collect();
    assert_eq!(keys, ["a", "b"], "sorted reads still work");
    let items = indexed.get(&"x".to_owned()).expect("get").expect("x");
    assert_eq!(items.query("n").eq(&7_u64).count().expect("count"), 1);
}

// ---------------------------------------------------------------------------
// Patched peers: forged entries are never read
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_forged_public_entry_in_an_owned_map_is_never_read() {
    let posts = alices_post();
    let collection = tags_of(&posts).id();
    let forged = forge_public_entry(collection, &"spam".to_owned(), &reg(666));

    // It is stored, as every node would store it...
    assert!(MainInterface::find_by_id_raw(forged).is_some());
    // ...and no read returns it.
    let tags = tags_of(&posts);
    assert_eq!(tag_names(&tags), ["rust"]);
    assert_eq!(tags.len().expect("len"), 1);
    assert!(tags.get("spam").expect("get").is_none());
    assert!(!tags.contains("spam").expect("contains"));
}

#[test]
#[serial]
fn a_forged_entry_cannot_overwrite_an_owned_one() {
    let posts = alices_post();
    let tags = tags_of(&posts);
    let existing = tags.entry_id("rust");
    let now = env::time_now() + 1_000_000_000;
    let action = Action::Update {
        id: existing,
        data: map_entry_bytes(existing, &"rust".to_owned(), &reg(666)),
        ancestors: vec![ChildInfo::new(tags.id(), [0; 32], Metadata::default())],
        metadata: Metadata {
            created_at: now,
            updated_at: now.into(),
            ..Metadata::default()
        },
    };
    assert!(
        MainInterface::apply_action(action, &ApplyContext::empty()).is_err(),
        "an owned entry's stamp cannot be downgraded"
    );
    assert_eq!(
        *tags_of(&posts)
            .get("rust")
            .expect("get")
            .expect("rust")
            .get(),
        1
    );
}

#[test]
#[serial]
fn an_entry_signed_by_someone_else_is_never_read() {
    let posts = alices_post();
    let collection = tags_of(&posts).id();
    let (mallory, _) = create_test_owner();
    let _ = forge_self_signed_entry(collection, &"mallory".to_owned(), &reg(666), &mallory);

    let tags = tags_of(&posts);
    assert_eq!(tag_names(&tags), ["rust"]);
    assert!(tags.get("mallory").expect("get").is_none());
}

#[test]
#[serial]
fn a_forged_entry_at_the_third_level_is_never_read() {
    type Leaf = UnorderedMap<String, LwwRegister<u64>>;
    type Deep = Authored<UnorderedMap<String, UnorderedMap<String, Leaf>>>;

    env::reset_for_testing();
    act_as(ALICE);
    let mut root = Root::new(Deep::new);
    let mut mid = UnorderedMap::new();
    let _ = mid.insert("m".to_owned(), Leaf::new()).expect("insert");
    root.insert("top".to_owned(), mid).expect("insert");

    let mid = root.get(&"top".to_owned()).expect("get").expect("top");
    let leaf = mid.get("m").expect("get").expect("m").into_inner();
    let _ = forge_public_entry(leaf.id(), &"spam".to_owned(), &reg(666));
    let _ = forge_public_entry(mid.id(), &"spam".to_owned(), &Leaf::new());

    let mid = root.get(&"top".to_owned()).expect("get").expect("top");
    let leaf = mid.get("m").expect("get").expect("m").into_inner();
    assert!(leaf.get("spam").expect("get").is_none());
    assert!(mid.get("spam").expect("get").is_none());
    assert_eq!(mid.len().expect("len"), 1);
}

#[test]
#[serial]
fn a_forged_entry_is_left_out_of_indexes_and_ordered_reads() {
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct Item(u64);
    impl Indexed for Item {
        const INDEXES: &'static [&'static str] = &["n"];
        fn index_keys(&self, _index: usize, out: &mut Vec<Vec<u8>>) {
            self.0.encode_index(out);
        }
    }

    env::reset_for_testing();
    act_as(ALICE);
    let mut indexed = Root::new(Authored::<UnorderedMap<String, IndexedMap<String, Item>>>::new);
    let mut items = IndexedMap::new();
    let _ = items.insert("i".to_owned(), Item(7)).expect("insert");
    indexed.insert("x".to_owned(), items).expect("insert");
    let mut sorted =
        Root::new(Authored::<UnorderedMap<String, SortedMap<String, LwwRegister<u64>>>>::new);
    let mut ordered = SortedMap::new();
    let _ = ordered.insert("b".to_owned(), reg(1)).expect("insert");
    sorted.insert("o".to_owned(), ordered).expect("insert");

    let items = indexed.get(&"x".to_owned()).expect("get").expect("x");
    let _ = forge_public_entry(items.id(), &"j".to_owned(), &Item(7));
    let ordered = sorted.get(&"o".to_owned()).expect("get").expect("o");
    let _ = forge_public_entry(ordered.id(), &"a".to_owned(), &reg(9));

    let items = indexed.get(&"x".to_owned()).expect("get").expect("x");
    assert_eq!(items.query("n").eq(&7_u64).count().expect("count"), 1);
    assert_eq!(items.len().expect("len"), 1);
    let ordered = sorted.get(&"o".to_owned()).expect("get").expect("o");
    let keys: Vec<_> = ordered.keys().expect("keys").collect();
    assert_eq!(keys, ["b"]);
}

// ---------------------------------------------------------------------------
// The policy wrappers' own entries
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn an_ownerless_entry_in_an_authored_map_is_never_read() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut notes = Root::new(Authored::<UnorderedMap<String, LwwRegister<u64>>>::new);
    notes.insert("mine".to_owned(), reg(1)).expect("insert");
    let _ = forge_public_entry((**notes).id(), &"ownerless".to_owned(), &reg(9));

    assert!(notes.get(&"ownerless".to_owned()).expect("get").is_none());
    assert_eq!(notes.len().expect("len"), 1);
    let keys: Vec<_> = notes.entries().expect("entries").map(|(k, _)| k).collect();
    assert_eq!(keys, ["mine"]);
}

#[test]
#[serial]
fn an_authored_map_inside_an_owned_entry_takes_everyone_s_entries() {
    type Comments = Authored<UnorderedMap<String, LwwRegister<u64>>>;

    env::reset_for_testing();
    act_as(ALICE);
    let mut threads = Root::new(Authored::<UnorderedMap<String, Comments>>::new);
    threads
        .insert("t".to_owned(), Comments::new())
        .expect("insert");

    // The inner authored map keeps its own policy: anyone may comment, and
    // each comment is owned by whoever wrote it.
    act_as(BOB);
    let mut comments = threads
        .get_by(&account(ALICE), &"t".to_owned())
        .expect("get")
        .expect("t");
    comments
        .insert("c1".to_owned(), reg(1))
        .expect("bob comments");
    assert_eq!(
        comments.owner_of(&"c1".to_owned()).expect("owner"),
        Some(account(BOB))
    );
}

#[test]
#[serial]
fn a_user_slot_signed_by_another_account_reads_as_empty() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut profiles = Root::new(UserStorage::<LwwRegister<u64>>::new);
    let _ = profiles.insert(reg(1)).expect("alice's slot");

    // Mallory signs an entry and parks it under Bob's key. It lands at her own
    // id for that key, not at Bob's slot.
    let (mallory, _) = create_test_owner();
    let bob = account(BOB);
    let _ = forge_self_signed_entry(profiles.inner_id(), &bob, &reg(666), &mallory);

    assert!(profiles.get_for_user(&bob).expect("get").is_none());
    assert!(!profiles.contains_user(&bob).expect("contains"));
    let owners: Vec<_> = profiles
        .entries()
        .expect("entries")
        .map(|(a, _)| a)
        .collect();
    assert_eq!(owners, [account(ALICE)]);

    // So nothing stands in Bob's way when he writes his own slot.
    act_as(BOB);
    let _ = profiles.insert(reg(2)).expect("bob's slot");
    assert_eq!(
        profiles.get_for_user(&bob).expect("get").map(|r| *r.get()),
        Some(2)
    );
}

#[test]
#[serial]
fn a_user_slot_s_nested_map_is_owned_by_the_slot_owner() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut stores = Root::new(UserStorage::<Tags>::new);
    let mut tags = Tags::new();
    let _ = tags.insert("a".to_owned(), reg(1)).expect("insert");
    let _ = stores.insert(tags).expect("alice's slot");

    let tags = stores.get().expect("get").expect("slot");
    assert!(is_owned_by(&stamp_of(tags.entry_id("a")), ALICE));

    act_as(BOB);
    let mut alices = stores
        .get_for_user(&account(ALICE))
        .expect("get")
        .expect("slot");
    assert!(refused(alices.insert("b".to_owned(), reg(2))));
}

#[test]
#[serial]
fn an_authored_vector_s_nested_map_is_owned_by_its_author() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut log = Root::new(AuthoredVector::<Tags>::new);
    let mut tags = Tags::new();
    let _ = tags.insert("a".to_owned(), reg(1)).expect("insert");
    log.push(tags).expect("push");

    let tags = log.get(0).expect("get").expect("entry");
    assert!(is_owned_by(&stamp_of(tags.entry_id("a")), ALICE));
    act_as(BOB);
    let mut alices = log.get(0).expect("get").expect("entry");
    assert!(refused(alices.insert("b".to_owned(), reg(2))));
}

#[test]
#[serial]
fn a_frozen_value_cannot_hold_entries_in_a_nested_collection() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut log = Root::new(ContentAddressed::<UnorderedMap<[u8; 32], Tags>>::new);

    let mut filled = Tags::new();
    let _ = filled.insert("a".to_owned(), reg(1)).expect("insert");
    assert!(
        log.insert(filled).is_err(),
        "a frozen value's bytes are its identity"
    );

    let hash = log.insert(Tags::new()).expect("an empty one is plain data");
    let mut empty = log.get(&hash).expect("get").expect("stored");
    assert!(
        refused(empty.insert("b".to_owned(), reg(2))),
        "and stays empty"
    );
    let _ = forge_public_entry(empty.id(), &"c".to_owned(), &reg(3));
    let empty = log.get(&hash).expect("get").expect("stored");
    assert!(empty.get("c").expect("get").is_none());
}

// ---------------------------------------------------------------------------
// Writer-set cells
// ---------------------------------------------------------------------------

type SharedTags = UnorderedMap<String, Tags>;

fn shared_cell() -> Root<WriterSetCell<SharedTags>> {
    env::reset_for_testing();
    act_as(ALICE);
    let writers = [account(ALICE)].into_iter().collect();
    let mut cell = Root::new(|| WriterSetCell::<SharedTags>::new(writers, false));
    let mut inner = Tags::new();
    let _ = inner.insert("x".to_owned(), reg(1)).expect("insert");
    let _ = cell
        .get_mut()
        .expect("writer")
        .insert("k".to_owned(), inner)
        .expect("insert");
    cell
}

#[test]
#[serial]
fn a_writer_set_guards_the_second_level_too() {
    let cell = shared_cell();
    let map = cell.get().expect("get");
    let inner = map.get("k").expect("get").expect("k").into_inner();
    let StorageType::SharedMember { anchor, .. } = stamp_of(map.entry_id("k")) else {
        panic!("level 1 is a member of the cell");
    };
    assert!(
        matches!(stamp_of(inner.entry_id("x")), StorageType::SharedMember { anchor: a, .. } if a == anchor),
        "level 2 is a member of the same cell"
    );

    // A non-writer's level-2 write carries the cell's member stamp, so the
    // cell's writer set decides it on every node that applies it.
    let mallory = SigningKey::from_bytes(&[0xB0; 32]);
    act_as(*account_of_key(&mallory).as_bytes());
    let map = cell.get().expect("get");
    let mut inner = map.get("k").expect("get").expect("k").into_inner();
    let _ = inner.insert("y".to_owned(), reg(2)).expect("lands locally");
    assert!(
        matches!(stamp_of(inner.entry_id("y")), StorageType::SharedMember { anchor: a, .. } if a == anchor),
        "a non-writer's level-2 write is stamped as a member of the cell"
    );

    let id = inner.entry_id("z");
    let action = build_signed_member_action(
        true,
        id,
        anchor,
        map_entry_bytes(id, &"z".to_owned(), &reg(3)),
        env::time_now() + 1_000_000_000,
        &mallory,
        vec![ChildInfo::new(inner.id(), [0; 32], Metadata::default())],
    );
    // A member write whose signer is outside the writer set is refused as
    // `InvalidSignature` (`sharedmember-signer-not-in-writer-set`).
    let applied = MainInterface::apply_action(action, &apply_ctx_for(account_of_key(&mallory)));
    assert!(
        matches!(applied, Err(StorageError::InvalidSignature)),
        "a peer refuses a non-writer's level-2 write: {applied:?}"
    );
    assert!(<Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .is_none());
    crate::tests::common::assert_every_shared_entity_is_bound();
}

#[test]
#[serial]
fn a_forged_entry_in_a_writer_set_cell_is_refused_and_never_read() {
    let cell = shared_cell();
    let map = cell.get().expect("get");
    let inner = map.get("k").expect("get").expect("k").into_inner();
    // Every id beneath a cell's value is bound to the cell, so the node refuses
    // a `Public` entry there outright (`tests/shared_occupation.rs`). The read
    // filter stays the second line.
    let (_, level_1) = try_public_entry(map.id(), &"spam".to_owned(), &Tags::new());
    let (_, level_2) = try_public_entry(inner.id(), &"spam".to_owned(), &reg(9));
    assert!(level_1.is_err() && level_2.is_err());

    let map = cell.get().expect("get");
    assert!(map.get("spam").expect("get").is_none(), "level 1");
    let inner = map.get("k").expect("get").expect("k").into_inner();
    assert!(inner.get("spam").expect("get").is_none(), "level 2");
    assert_eq!(inner.len().expect("len"), 1);
}
