//! A query can return hits by a number field instead of by relevance: the
//! newest messages that match, paged in that order.

use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchMode, SearchOrder,
    SearchRequest, SearchValue,
};
use calimero_search::{SearchConfig, SearchService};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use std::sync::Arc;

fn schema() -> SearchIndexSchema {
    SearchIndexSchema {
        name: "messages".to_owned(),
        version: 1,
        fields: vec![
            SearchFieldSchema {
                name: "text".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: false,
                },
            },
            SearchFieldSchema {
                name: "ts".to_owned(),
                kind: SearchFieldKind::U64,
            },
            SearchFieldSchema {
                name: "room".to_owned(),
                kind: SearchFieldKind::Keyword,
            },
        ],
    }
}

fn doc(n: u8, text: &str, ts: u64) -> SearchDoc {
    SearchDoc {
        id: [n; 32],
        fields: vec![
            ("text".to_owned(), SearchValue::Str(text.to_owned())),
            ("ts".to_owned(), SearchValue::U64(ts)),
            ("room".to_owned(), SearchValue::Str("r".to_owned())),
        ],
    }
}

fn request(order: SearchOrder, cursor: u32, limit: u32) -> SearchRequest {
    SearchRequest {
        index: "messages".to_owned(),
        query: "hello".to_owned(),
        mode: SearchMode::Words,
        filters: vec![],
        order,
        cursor,
        limit,
    }
}

fn by_ts(descending: bool) -> SearchOrder {
    SearchOrder::Field {
        field: "ts".to_owned(),
        descending,
    }
}

#[test]
fn hits_come_back_by_a_number_field_and_page_in_that_order() {
    let service = SearchService::new(
        Store::new(Arc::new(InMemoryDB::owned())),
        SearchConfig::default(),
    );
    let (index, _) = service.open_index(&[7; 32], &schema()).unwrap();
    // Relevance would put the doc that says "hello" twice first.
    let docs = [
        doc(1, "hello hello hello", 10),
        doc(2, "hello", 40),
        doc(3, "hello there", 30),
        doc(4, "goodbye", 50),
        doc(5, "hello again", 20),
    ];
    let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d)))).unwrap();
    index.commit(1, [0; 32]).unwrap();

    let ids = |order: SearchOrder, cursor: u32, limit: u32| {
        let page = index.search(&request(order, cursor, limit)).unwrap();
        (
            page.hits.iter().map(|h| h.id[0]).collect::<Vec<_>>(),
            page.next_cursor,
            page.total,
        )
    };
    assert_eq!(
        ids(SearchOrder::Relevance, 0, 10).0[0],
        1,
        "relevance first"
    );
    assert_eq!(ids(by_ts(true), 0, 10), (vec![2, 3, 5, 1], None, 4));
    assert_eq!(ids(by_ts(false), 0, 10).0, vec![1, 5, 3, 2]);
    let (first, next, _) = ids(by_ts(true), 0, 2);
    assert_eq!(first, vec![2, 3]);
    assert_eq!(ids(by_ts(true), next.unwrap(), 2).0, vec![5, 1], "page two");

    let not_a_number = SearchOrder::Field {
        field: "room".to_owned(),
        descending: true,
    };
    assert!(index.search(&request(not_a_number, 0, 10)).is_err());
    let unknown = SearchOrder::Field {
        field: "nope".to_owned(),
        descending: true,
    };
    assert!(index.search(&request(unknown, 0, 10)).is_err());
}

/// A hit's snippet comes from the text field its words were found in, not
/// always the first one.
#[test]
fn a_snippet_comes_from_the_field_that_matched() {
    let schema = SearchIndexSchema {
        name: "docs".to_owned(),
        version: 1,
        fields: vec![
            SearchFieldSchema {
                name: "title".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 200,
                    infix: false,
                },
            },
            SearchFieldSchema {
                name: "body".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: false,
                },
            },
        ],
    };
    let service = SearchService::new(
        Store::new(Arc::new(InMemoryDB::owned())),
        SearchConfig::default(),
    );
    let (index, _) = service.open_index(&[8; 32], &schema).unwrap();
    let doc = SearchDoc {
        id: [1; 32],
        fields: vec![
            (
                "title".to_owned(),
                SearchValue::Str("Quarterly plan".to_owned()),
            ),
            (
                "body".to_owned(),
                SearchValue::Str("the budget is final".to_owned()),
            ),
        ],
    };
    let _ = index.apply([(&doc.id, Some(&doc))]).unwrap();
    index.commit(1, [0; 32]).unwrap();
    let snippet = |q: &str| {
        let mut req = request(SearchOrder::Relevance, 0, 10);
        req.index = "docs".to_owned();
        req.query = q.to_owned();
        index.search(&req).unwrap().hits[0].snippet.clone()
    };
    assert!(
        snippet("budget").contains("<b>budget</b>"),
        "{}",
        snippet("budget")
    );
    assert!(
        snippet("plan").contains("<b>plan</b>"),
        "{}",
        snippet("plan")
    );
}

/// A query typed as you go marks the word its last, unfinished word
/// completes to, and every finished word before it, in the field they
/// matched.
#[test]
fn a_prefix_query_marks_the_words_it_completed() {
    let schema = SearchIndexSchema {
        name: "docs".to_owned(),
        version: 1,
        fields: vec![
            SearchFieldSchema {
                name: "title".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 200,
                    infix: false,
                },
            },
            SearchFieldSchema {
                name: "body".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: false,
                },
            },
        ],
    };
    let service = SearchService::new(
        Store::new(Arc::new(InMemoryDB::owned())),
        SearchConfig::default(),
    );
    let (index, _) = service.open_index(&[9; 32], &schema).unwrap();
    let doc = SearchDoc {
        id: [1; 32],
        fields: vec![
            (
                "title".to_owned(),
                SearchValue::Str("Quarterly plan".to_owned()),
            ),
            (
                "body".to_owned(),
                SearchValue::Str("One breaking change per year, and zucchini.".to_owned()),
            ),
        ],
    };
    let _ = index.apply([(&doc.id, Some(&doc))]).unwrap();
    index.commit(1, [0; 32]).unwrap();
    let snippet = |q: &str| {
        let mut req = request(SearchOrder::Relevance, 0, 10);
        req.index = "docs".to_owned();
        req.query = q.to_owned();
        req.mode = SearchMode::Prefix;
        index.search(&req).unwrap().hits[0].snippet.clone()
    };
    // A finished word, and the same word half typed.
    assert!(
        snippet("zucchini").contains("<b>zucchini</b>"),
        "{}",
        snippet("zucchini")
    );
    assert!(
        snippet("zucc").contains("<b>zucchini</b>"),
        "{}",
        snippet("zucc")
    );
    // Each word on its own, the last one completed.
    let both = snippet("breaking chan");
    assert!(
        both.contains("<b>breaking</b>") && both.contains("<b>change</b>"),
        "{both}"
    );
    // A title match is shown in the title.
    assert!(
        snippet("quart").contains("<b>Quarterly</b>"),
        "{}",
        snippet("quart")
    );
}
