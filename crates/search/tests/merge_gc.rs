//! A reader must keep working across the writer's merges and garbage
//! collection: segment files a live searcher still reads must not vanish.

use std::sync::Arc;

use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchMode, SearchOrder,
    SearchRequest, SearchValue,
};
use calimero_search::{SearchConfig, SearchService};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;

fn schema() -> SearchIndexSchema {
    SearchIndexSchema {
        name: "messages".to_owned(),
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

fn run(store: Store) {
    let service = SearchService::new(store, SearchConfig::default());
    let (index, _) = service.open_index(&[7; 32], &schema()).unwrap();
    // One document up front that the first batch then replaces: the replace
    // is a delete, so segments carry delete files through the merges.
    let first = SearchDoc {
        id: {
            let mut id = [0; 32];
            id[..4].copy_from_slice(&1_u32.to_be_bytes());
            id
        },
        fields: vec![("text".to_owned(), SearchValue::Str("early".to_owned()))],
    };
    let _ = index.apply([(&first.id, Some(&first))]).unwrap();
    let mut n = 0_u32;
    for batch in 0..12 {
        let docs: Vec<SearchDoc> = (0..1_000)
            .map(|_| {
                n += 1;
                let mut id = [0; 32];
                id[..4].copy_from_slice(&n.to_be_bytes());
                SearchDoc {
                    id,
                    fields: vec![(
                        "text".to_owned(),
                        SearchValue::Str(format!("common word{} batch{batch}", n % 97)),
                    )],
                }
            })
            .collect();
        let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d)))).unwrap();
        index.commit(1, [0; 32]).unwrap();
    }
    index.close_writer().unwrap();
    let q = SearchRequest {
        index: "messages".to_owned(),
        query: "common".to_owned(),
        mode: SearchMode::Words,
        filters: vec![],
        order: SearchOrder::Relevance,
        cursor: 0,
        limit: 20,
    };
    assert_eq!(index.search(&q).unwrap().total, 12_000);
    drop(index);
    drop(service);
}

#[test]
fn searches_survive_merges_and_writer_close_in_memory() {
    run(Store::new(Arc::new(InMemoryDB::owned())));
}

#[test]
fn searches_survive_merges_and_writer_close_on_rocksdb() {
    let dir = tempfile::tempdir().unwrap();
    let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    run(
        Store::open::<calimero_store_rocksdb::RocksDB>(&calimero_store::config::StoreConfig::new(
            path,
        ))
        .unwrap(),
    );
}
