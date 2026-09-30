//! What a search index reads back through each collection that can be one.
//!
//! The index hands a view entity ids, and the node's full build pages through
//! a collection by id, so every [`SearchCollection`] must return, for an id,
//! exactly the entry a read of that collection returns: every owner's entry of
//! an owned collection, and nothing for an id of any other collection, however
//! the store resolves it.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::search::{SearchCollection, SearchFieldSchema, SearchValue, Searchable};
use calimero_storage::address::Id;
use calimero_storage::collections::{
    AuthoredMap, AuthoredVector, IndexedMap, Moderated, SortedMap, UnorderedMap, WriteOnce,
};
use calimero_storage::env;
use serial_test::serial;

const ALICE: [u8; 32] = [0xA1; 32];
const BOB: [u8; 32] = [0xB0; 32];

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Note(String);

impl Searchable for Note {
    fn search_fields() -> Vec<SearchFieldSchema> {
        Vec::new()
    }

    fn search_document(&self) -> Vec<(String, SearchValue)> {
        vec![("text".to_owned(), SearchValue::Str(self.0.clone()))]
    }
}

impl calimero_storage::collections::Indexed for Note {
    const INDEXES: &'static [&'static str] = &[];

    fn index_keys(&self, _index: usize, _out: &mut Vec<Vec<u8>>) {}
}

/// Every id a full build pages through, in pages of `at_least`.
fn every_id<C: SearchCollection>(collection: &C, at_least: usize) -> Vec<[u8; 32]> {
    let mut ids = Vec::new();
    let mut from = Some([0; 32]);
    while let Some(bound) = from {
        let (page, next) = collection.search_page(bound, at_least).expect("page");
        ids.extend(page);
        from = next;
    }
    ids
}

/// The value of every entry the build pages through, read back by id.
fn every_value<C>(collection: &C) -> Vec<Note>
where
    C: SearchCollection<Value = Note>,
{
    let mut values: Vec<Note> = every_id(collection, 2)
        .into_iter()
        .map(|id| {
            collection
                .search_entry(id)
                .expect("read")
                .expect("a paged id reads back")
                .1
        })
        .collect();
    values.sort_by(|a, b| a.0.cmp(&b.0));
    values
}

fn notes(texts: &[&str]) -> Vec<Note> {
    texts.iter().map(|t| Note((*t).to_owned())).collect()
}

/// Two accounts write the same key; both entries are found, each by its id.
fn two_owners_one_key<C>(mut collection: C, insert: impl Fn(&mut C, &str, Note)) -> C
where
    C: SearchCollection<Value = Note>,
{
    env::with_account_id(ALICE, || {
        insert(&mut collection, "shared", Note("alice".to_owned()));
        insert(&mut collection, "only-alice", Note("alice-2".to_owned()));
    });
    env::with_account_id(BOB, || {
        insert(&mut collection, "shared", Note("bob".to_owned()));
    });
    assert_eq!(
        every_value(&collection),
        notes(&["alice", "alice-2", "bob"]),
        "every owner's entry is paged and read back"
    );
    collection
}

#[test]
#[serial]
fn an_authored_map_reads_back_every_owners_entry() {
    env::reset_environment();
    let map = two_owners_one_key(AuthoredMap::<String, Note>::new(), |m, k, v| {
        m.insert(k.to_owned(), v).expect("insert")
    });
    let (key, _) = map
        .search_entry(every_id(&map, 10)[0])
        .expect("read")
        .expect("entry");
    assert!(
        key == "shared" || key == "only-alice",
        "the key comes back too"
    );
}

#[test]
#[serial]
fn a_moderated_sorted_map_reads_back_every_owners_entry() {
    env::reset_environment();
    let _map = two_owners_one_key(Moderated::<SortedMap<String, Note>>::new(), |m, k, v| {
        m.insert(k.to_owned(), v).expect("insert")
    });
}

#[test]
#[serial]
fn a_write_once_map_and_an_indexed_one_read_back_every_owners_entry() {
    env::reset_environment();
    let _once = two_owners_one_key(WriteOnce::<UnorderedMap<String, Note>>::new(), |m, k, v| {
        m.insert(k.to_owned(), v).expect("insert")
    });
    let _indexed = two_owners_one_key(Moderated::<IndexedMap<String, Note>>::new(), |m, k, v| {
        m.insert(k.to_owned(), v).expect("insert")
    });
}

#[test]
#[serial]
fn a_plain_map_and_a_sorted_map_read_back_their_entries() {
    env::reset_environment();
    let mut map = UnorderedMap::<String, Note>::new();
    let mut sorted = SortedMap::<String, Note>::new();
    for text in ["a", "b", "c"] {
        let _ = map
            .insert(text.to_owned(), Note(text.to_owned()))
            .expect("insert");
        let _ = sorted
            .insert(text.to_owned(), Note(text.to_owned()))
            .expect("insert");
    }
    assert_eq!(every_value(&map), notes(&["a", "b", "c"]));
    assert_eq!(every_value(&sorted), notes(&["a", "b", "c"]));
}

#[test]
#[serial]
fn an_authored_vector_reads_back_every_owners_entry_by_id() {
    env::reset_environment();
    let mut vector = AuthoredVector::<Note>::new();
    let a = env::with_account_id(ALICE, || {
        vector.push(Note("alice".to_owned())).expect("push")
    });
    let b = env::with_account_id(BOB, || vector.push(Note("bob".to_owned())).expect("push"));

    assert_eq!(every_value(&vector), notes(&["alice", "bob"]));
    let key = <[u8; 32]>::from(a);
    assert_eq!(
        vector.search_entry(key).expect("read"),
        Some((key, Note("alice".to_owned()))),
        "the key is the entity id"
    );
    assert!(vector
        .search_entry(<[u8; 32]>::from(b))
        .expect("read")
        .is_some());
    assert_eq!(vector.position_of_id(a).expect("position"), Some(0));
    assert_eq!(vector.position_of_id(b).expect("position"), Some(1));
    assert_eq!(
        vector.get(1).expect("get"),
        Some(Note("bob".to_owned())),
        "the position reads the entry back"
    );
}

/// An id of any other collection reads as nothing, whether the store holds
/// an entity there of the same value type or of another.
#[test]
#[serial]
fn an_id_of_another_collection_reads_back_as_nothing() {
    env::reset_environment();
    let mut one = AuthoredMap::<String, Note>::new();
    let mut other = AuthoredMap::<String, Note>::new();
    let mut plain = UnorderedMap::<String, Note>::new();
    let mut vector = AuthoredVector::<Note>::new();
    let mut other_vector = AuthoredVector::<Note>::new();
    one.insert("k".to_owned(), Note("one".to_owned()))
        .expect("insert");
    other
        .insert("k".to_owned(), Note("other".to_owned()))
        .expect("insert");
    let _ = plain
        .insert("k".to_owned(), Note("plain".to_owned()))
        .expect("insert");
    let _ = vector.push(Note("vector".to_owned())).expect("push");
    let _ = other_vector
        .push(Note("other vector".to_owned()))
        .expect("push");

    let foreign: Vec<[u8; 32]> = [
        every_id(&other, 10),
        every_id(&plain, 10),
        every_id(&other_vector, 10),
    ]
    .concat();
    assert_eq!(foreign.len(), 3);
    for id in &foreign {
        assert_eq!(one.search_entry(*id).expect("read"), None);
        assert_eq!(vector.search_entry(*id).expect("read"), None);
    }
    let own_map = every_id(&one, 10)[0];
    assert_eq!(plain.search_entry(own_map).expect("read"), None);
    assert_eq!(vector.search_entry(own_map).expect("read"), None);
    assert_eq!(
        one.search_entry(Id::new([7; 32]).into()).expect("read"),
        None,
        "an id nothing lives at"
    );
}

/// Collaborative text indexes as the text a reader sees: a `FugueText` its
/// characters, a `RichDocument` every live block's text in block order.
#[test]
#[serial]
fn rich_text_indexes_as_its_visible_text() {
    use calimero_sdk::search::SearchText;
    use calimero_storage::collections::{DefaultMarks, DeltaOp, FugueText, RichDocument};

    env::reset_environment();
    let mut title = FugueText::new();
    let _ = title.insert_str(0, "Road map").expect("insert");
    assert_eq!(title.search_text().as_deref(), Some("Road map"));

    let mut doc = RichDocument::<DefaultMarks>::new();
    let first = doc.insert_block(None, "p", 0).expect("block");
    let second = doc.insert_block(Some(first), "p", 0).expect("block");
    let gone = doc.insert_block(Some(second), "p", 0).expect("block");
    let _ = doc
        .apply_delta(first, &[DeltaOp::insert("first line")])
        .expect("text");
    let _ = doc
        .apply_delta(second, &[DeltaOp::insert("second")])
        .expect("text");
    let _ = doc
        .apply_delta(gone, &[DeltaOp::insert("deleted")])
        .expect("text");
    assert!(
        !doc.delete_block(gone).expect("delete"),
        "not deleted before"
    );
    assert_eq!(
        doc.search_text().as_deref(),
        Some("first line\nsecond"),
        "live blocks in order, one line each, a deleted one left out"
    );
}
