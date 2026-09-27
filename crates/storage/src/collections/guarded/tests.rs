use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use serial_test::serial;
use sha2::{Digest, Sha256};

use super::{Authored, ContentAddressed};
use crate::collections::{
    AuthoredMap, FrozenStorage, IndexValue, Indexed, IndexedMap, Root, UnorderedMap,
};
use crate::entities::{Data, StorageType};
use crate::env;
use crate::index::Index;
use crate::store::MainStorage;

const ALICE: [u8; 32] = [0x11; 32];
const BOB: [u8; 32] = [0x22; 32];

/// A post, indexed by board.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
struct Post {
    board: String,
    title: String,
}

impl Indexed for Post {
    const INDEXES: &'static [&'static str] = &["board"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        if index == 0 {
            self.board.encode_index(out);
        }
    }
}

fn post(board: &str, title: &str) -> Post {
    Post {
        board: board.to_owned(),
        title: title.to_owned(),
    }
}

type AuthoredPosts = Authored<IndexedMap<String, Post>>;

fn titles(map: &AuthoredPosts, board: &str) -> Vec<String> {
    let mut titles: Vec<_> = map
        .query("board")
        .eq(board)
        .entries()
        .expect("query")
        .into_iter()
        .map(|(_, p)| p.title)
        .collect();
    titles.sort();
    titles
}

#[test]
#[serial]
fn an_authored_indexed_map_gates_writes_and_keeps_its_indexes() {
    env::reset_for_testing();
    env::set_account_id(ALICE);
    let mut posts = Root::new(AuthoredPosts::new);
    posts
        .insert("p1".to_owned(), post("dev", "one"))
        .expect("insert");
    posts
        .insert("p2".to_owned(), post("dev", "two"))
        .expect("insert");
    assert_eq!(titles(&posts, "dev"), ["one", "two"]);
    assert_eq!(
        posts.owner_of(&"p1".to_owned()).expect("owner"),
        Some(AccountId::from(ALICE))
    );

    env::set_account_id(BOB);
    assert!(posts.insert("p1".to_owned(), post("ops", "taken")).is_err());
    assert!(posts
        .modify(&"p1".to_owned(), |p| p.board = "ops".to_owned())
        .is_err());
    assert!(posts.remove(&"p2".to_owned()).is_err());
    assert_eq!(titles(&posts, "dev"), ["one", "two"]);

    env::set_account_id(ALICE);
    posts
        .modify(&"p1".to_owned(), |p| p.board = "ops".to_owned())
        .expect("owner moves p1");
    assert_eq!(titles(&posts, "dev"), ["two"]);
    assert_eq!(titles(&posts, "ops"), ["one"]);
    assert_eq!(
        posts.remove(&"p2".to_owned()).expect("owner removes"),
        Some(post("dev", "two"))
    );
    assert!(titles(&posts, "dev").is_empty());
    assert_eq!(posts.query("board").eq("ops").count().expect("count"), 1);
}

#[test]
#[serial]
fn an_absent_key_is_not_found_rather_than_refused() {
    env::reset_for_testing();
    env::set_account_id(ALICE);
    let mut posts = Root::new(AuthoredPosts::new);
    assert!(posts.update(&"nope".to_owned(), post("dev", "x")).is_err());
    assert_eq!(posts.remove(&"nope".to_owned()).expect("remove"), None);
}

/// `Authored<IndexedMap>` and `AuthoredMap` are one layout: bytes written by
/// either read back through the other, owner stamps included. Switching a field
/// between them needs no migration.
#[test]
#[serial]
fn authored_indexed_map_and_authored_map_are_one_layout() {
    env::reset_for_testing();
    env::set_account_id(ALICE);

    let mut indexed = AuthoredPosts::new_with_field_name("posts");
    indexed
        .insert("p1".to_owned(), post("dev", "one"))
        .expect("insert");
    let plain: AuthoredMap<String, Post> =
        borsh::from_slice(&borsh::to_vec(&indexed).expect("serialize")).expect("deserialize");
    assert_eq!(plain.id(), indexed.id());
    assert_eq!(
        plain.get(&"p1".to_owned()).expect("get"),
        Some(post("dev", "one"))
    );
    assert_eq!(
        plain.owner_of(&"p1".to_owned()).expect("owner"),
        Some(AccountId::from(ALICE))
    );

    let mut plain = AuthoredMap::<String, Post>::new_with_field_name("more");
    plain
        .insert("p2".to_owned(), post("ops", "two"))
        .expect("insert");
    let indexed: AuthoredPosts =
        borsh::from_slice(&borsh::to_vec(&plain).expect("serialize")).expect("deserialize");
    assert_eq!(titles(&indexed, "ops"), ["two"]);
    assert!(indexed.owned_by_me(&"p2".to_owned()).expect("owned"));

    // The same field name gives the same ids either way. `new_with_field_name`
    // leaves the wrapper id random until the post-init reassign, as it always has.
    let mut a = AuthoredPosts::new();
    a.reassign_deterministic_id("same");
    let mut b = AuthoredMap::<String, Post>::new();
    b.reassign_deterministic_id("same");
    assert_eq!(a.id(), b.id());
    assert_eq!(
        <IndexedMap<String, Post> as Data>::id(&a),
        <UnorderedMap<String, Post> as Data>::id(&b)
    );
}

type Log = ContentAddressed<IndexedMap<[u8; 32], Post>>;

#[test]
#[serial]
fn a_frozen_collection_is_keyed_by_content_and_stamped_frozen() {
    env::reset_for_testing();
    let mut log = Root::new(Log::new);
    let first = log.insert(post("dev", "one")).expect("insert");
    let expected: [u8; 32] =
        Sha256::digest(borsh::to_vec(&post("dev", "one")).expect("bytes")).into();
    assert_eq!(first, expected);
    assert_eq!(log.insert(post("dev", "one")).expect("again"), first);
    let _ = log.insert(post("dev", "two")).expect("insert");

    assert_eq!(log.get(&first).expect("get"), Some(post("dev", "one")));
    assert_eq!(log.query("board").eq("dev").count().expect("count"), 2);
    assert_eq!(log.len().expect("len"), 2);

    let metadata = <Index<MainStorage>>::get_metadata(log.entry_id(&first))
        .expect("metadata")
        .expect("present");
    assert!(matches!(metadata.storage_type, StorageType::Frozen));
}

/// `ContentAddressed<UnorderedMap<Hash, T>>` stores exactly a `FrozenStorage<T>`'s bytes.
#[test]
#[serial]
fn frozen_unordered_map_and_frozen_storage_are_one_layout() {
    env::reset_for_testing();
    let mut frozen = ContentAddressed::<UnorderedMap<[u8; 32], String>>::new_with_field_name("log");
    let hash = frozen.insert("hello".to_owned()).expect("insert");

    let storage: FrozenStorage<String> =
        borsh::from_slice(&borsh::to_vec(&frozen).expect("serialize")).expect("deserialize");
    assert_eq!(storage.get(&hash).expect("get"), Some("hello".to_owned()));

    let mut storage = FrozenStorage::<String>::new_with_field_name("other");
    let hash = storage.insert("world".to_owned()).expect("insert");
    let frozen: ContentAddressed<UnorderedMap<[u8; 32], String>> =
        borsh::from_slice(&borsh::to_vec(&storage).expect("serialize")).expect("deserialize");
    assert_eq!(frozen.get(&hash).expect("get"), Some("world".to_owned()));
}

/// Generic code over `AuthoredMap<K, V>` reads an owner without bounding
/// `V: 'static`, as it could before `AuthoredMap` became `Guarded`: apps in
/// the wild (mero-chess) are written that way.
fn owner_of_any<V: BorshSerialize + BorshDeserialize>(
    map: &AuthoredMap<String, V>,
    key: &String,
) -> Option<AccountId> {
    let _mine = map.owned_by_me(key).expect("owned_by_me");
    let _version = map.entry_schema_version(key).expect("schema version");
    map.owner_of(key).expect("owner")
}

#[test]
#[serial]
fn an_owner_reads_back_through_code_generic_over_the_value() {
    env::reset_for_testing();
    env::set_account_id(ALICE);
    let mut posts = Root::new(AuthoredMap::<String, Post>::new);
    posts
        .insert("p1".to_owned(), post("dev", "one"))
        .expect("insert");
    assert_eq!(
        owner_of_any(&posts, &"p1".to_owned()),
        Some(AccountId::from(ALICE))
    );
    assert_eq!(owner_of_any(&posts, &"absent".to_owned()), None);
}
