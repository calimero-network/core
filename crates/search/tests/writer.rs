//! An index holds a writer (its threads and arena) only while something writes:
//! opening leaves none, and one closed is opened again by the next write.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]
use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchMode, SearchOrder,
    SearchRequest, SearchValue,
};
use calimero_search::index::{ContextIndex, SchemaOptions};

fn schema() -> SearchIndexSchema {
    SearchIndexSchema {
        name: "notes".to_owned(),
        version: 1,
        fields: vec![SearchFieldSchema {
            name: "text".to_owned(),
            kind: SearchFieldKind::Text {
                weight: 100,
                infix: true,
            },
        }],
    }
}

fn doc(n: u8, text: &str) -> SearchDoc {
    SearchDoc {
        id: [n; 32],
        fields: vec![("text".to_owned(), SearchValue::Str(text.to_owned()))],
    }
}

fn total(index: &ContextIndex, text: &str) -> u64 {
    index
        .search(&SearchRequest {
            index: "notes".to_owned(),
            query: text.to_owned(),
            mode: SearchMode::Words,
            filters: vec![],
            order: SearchOrder::Relevance,
            cursor: 0,
            limit: 10,
        })
        .unwrap()
        .total
}

#[test]
fn a_writer_is_open_only_between_a_write_and_its_close() {
    let dir = Box::new(tantivy::directory::RamDirectory::create());
    let (index, _state) = ContextIndex::open(dir, &schema(), SchemaOptions::default()).unwrap();
    let index = index.expect("a new index opens fresh");
    assert!(!index.has_writer(), "opening a new index leaves no writer");

    for round in 0..3_u8 {
        let d = doc(round, &format!("hello round{round}"));
        let _ = index.apply([(&d.id, Some(&d))]).unwrap();
        assert!(index.has_writer());
        index.commit(u64::from(round) + 1, [0; 32]).unwrap();
        index.close_writer().unwrap();
        assert!(!index.has_writer());
        assert_eq!(total(&index, "hello"), u64::from(round) + 1);
        assert_eq!(index.committed_seq(), u64::from(round) + 1);
    }

    // A replace after a close deletes what an earlier writer wrote.
    let d = doc(0, "goodbye");
    let _ = index.apply([(&d.id, Some(&d))]).unwrap();
    index.commit(9, [0; 32]).unwrap();
    index.close_writer().unwrap();
    assert_eq!(total(&index, "hello"), 2);
    assert_eq!(total(&index, "goodbye"), 1);
}

/// A released writer finishes its merges on its own thread; writing again waits for
/// it (it holds the index's lock), and what it did not commit goes with it.
#[test]
fn a_released_writer_is_opened_again_by_the_next_write() {
    let dir = Box::new(tantivy::directory::RamDirectory::create());
    let (index, _state) = ContextIndex::open(dir, &schema(), SchemaOptions::default()).unwrap();
    let index = index.expect("a new index opens fresh");

    // Enough commits for the merge policy to merge while writers come and go.
    for round in 0..20_u8 {
        let d = doc(round, &format!("hello round{round}"));
        let _ = index.apply([(&d.id, Some(&d))]).unwrap();
        index.commit(u64::from(round) + 1, [0; 32]).unwrap();
        index.release_writer().unwrap();
        assert!(!index.has_writer());
    }
    assert_eq!(total(&index, "hello"), 20);

    // Uncommitted changes go with the writer released.
    let d = doc(99, "never committed");
    let _ = index.apply([(&d.id, Some(&d))]).unwrap();
    index.release_writer().unwrap();
    index.commit(21, [0; 32]).unwrap();
    index.close_writer().unwrap();
    assert_eq!(total(&index, "committed"), 0);
    assert_eq!(total(&index, "hello"), 20);

    // Dropped while a released writer is still closing.
    let _ = index.apply([(&d.id, Some(&d))]).unwrap();
    index.commit(22, [0; 32]).unwrap();
    index.release_writer().unwrap();
    drop(index);
}

/// A searcher reads the commit it was taken at: later commits, deletes and the merges a
/// released writer finishes leave what it answers as it was.
#[test]
fn a_searcher_reads_the_commit_it_was_taken_at() {
    let dir = Box::new(tantivy::directory::RamDirectory::create());
    let (index, _state) = ContextIndex::open(dir, &schema(), SchemaOptions::default()).unwrap();
    let index = index.expect("a new index opens fresh");
    let (one, two) = (doc(1, "hello one"), doc(2, "hello two"));
    let _ = index
        .apply([(&one.id, Some(&one)), (&two.id, Some(&two))])
        .unwrap();
    index.commit(1, [0; 32]).unwrap();
    let at1 = index.searcher();

    let three = doc(3, "hello three");
    let _ = index
        .apply([(&two.id, None), (&three.id, Some(&three))])
        .unwrap();
    index.commit(2, [0; 32]).unwrap();
    index.close_writer().unwrap();

    let request = SearchRequest {
        index: "notes".to_owned(),
        query: "hello".to_owned(),
        mode: SearchMode::Words,
        filters: vec![],
        order: SearchOrder::Relevance,
        cursor: 0,
        limit: 10,
    };
    let ids = |r: calimero_primitives::search::SearchResponse| {
        let mut ids: Vec<u8> = r.hits.iter().map(|h| h.id[0]).collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(ids(index.search_in(&at1, &request).unwrap()), vec![1, 2]);
    assert_eq!(ids(index.search(&request).unwrap()), vec![1, 3]);
}
