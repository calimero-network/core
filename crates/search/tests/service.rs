//! The service's contract: per-context isolation, per-context scoring,
//! idempotent replay, rebuild on a schema change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use calimero_primitives::search::{
    ExtractResponse, ScanResponse, SearchDoc, SearchFieldKind, SearchFieldSchema, SearchFilter,
    SearchIndexSchema, SearchMode, SearchRequest, SearchValue,
};
use calimero_search::{dirty, ContextKey, Extractor, SearchConfig, SearchService};
use calimero_store::db::InMemoryDB;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

const A: ContextKey = [0xA; 32];
const B: ContextKey = [0xB; 32];

fn schema(version: u32) -> SearchIndexSchema {
    SearchIndexSchema {
        name: "messages".to_owned(),
        version,
        fields: vec![
            SearchFieldSchema {
                name: "text".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: true,
                },
            },
            SearchFieldSchema {
                name: "sender".to_owned(),
                kind: SearchFieldKind::Keyword,
            },
        ],
    }
}

fn doc(context: ContextKey, i: u32, text: &str, sender: &str) -> SearchDoc {
    let mut id = [0_u8; 32];
    id[0] = context[0];
    id[1..5].copy_from_slice(&i.to_be_bytes());
    SearchDoc {
        id,
        fields: vec![
            ("text".to_owned(), SearchValue::Str(text.to_owned())),
            ("sender".to_owned(), SearchValue::Str(sender.to_owned())),
        ],
    }
}

fn query(q: &str, mode: SearchMode) -> SearchRequest {
    SearchRequest {
        index: "messages".to_owned(),
        query: q.to_owned(),
        mode,
        filters: vec![],
        cursor: 0,
        limit: 100,
    }
}

/// Per-context app state, as the node's extractor would see it.
#[derive(Default)]
struct App {
    state: Mutex<HashMap<ContextKey, HashMap<[u8; 32], SearchDoc>>>,
    version: AtomicUsize,
    scans: AtomicUsize,
    fail_extract: AtomicUsize,
}

impl App {
    fn put(&self, store: &Store, context: ContextKey, doc: SearchDoc) {
        let mut tx = Transaction::default();
        let _ = dirty::stage(&mut tx, &context, &[doc.id]).unwrap();
        store.apply(&tx).unwrap();
        let _ = self
            .state
            .lock()
            .unwrap()
            .entry(context)
            .or_default()
            .insert(doc.id, doc);
    }

    fn delete(&self, store: &Store, context: ContextKey, id: [u8; 32]) {
        let mut tx = Transaction::default();
        let _ = dirty::stage(&mut tx, &context, &[id]).unwrap();
        store.apply(&tx).unwrap();
        let _ = self
            .state
            .lock()
            .unwrap()
            .entry(context)
            .or_default()
            .remove(&id);
    }
}

#[async_trait]
impl Extractor for App {
    async fn schema(&self, _: ContextKey) -> EyreResult<Option<Vec<SearchIndexSchema>>> {
        Ok(Some(vec![schema(
            self.version.load(Ordering::SeqCst) as u32
        )]))
    }

    async fn extract(
        &self,
        c: ContextKey,
        _: &str,
        ids: Vec<[u8; 32]>,
    ) -> EyreResult<ExtractResponse> {
        if self.fail_extract.load(Ordering::SeqCst) > 0 {
            let _ = self.fail_extract.fetch_sub(1, Ordering::SeqCst);
            bail!("simulated crash");
        }
        let state = self.state.lock().unwrap();
        let docs = state.get(&c);
        Ok(ids
            .iter()
            .map(|id| docs.and_then(|d| d.get(id)).cloned())
            .collect())
    }

    async fn scan(
        &self,
        c: ContextKey,
        _: &str,
        offset: u32,
        limit: u32,
    ) -> EyreResult<ScanResponse> {
        let _ = self.scans.fetch_add(1, Ordering::SeqCst);
        let state = self.state.lock().unwrap();
        let mut all: Vec<SearchDoc> = state
            .get(&c)
            .map(|d| d.values().cloned().collect())
            .unwrap_or_default();
        all.sort_by_key(|d| d.id);
        let docs: Vec<SearchDoc> = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        let next = (docs.len() == limit as usize).then_some(offset + limit);
        Ok(ScanResponse { docs, next })
    }
}

fn setup() -> (Store, Arc<SearchService>, App) {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let service = SearchService::new(store.clone(), SearchConfig::default());
    (store, service, App::default())
}

#[tokio::test]
async fn a_context_never_sees_another_contexts_hits() {
    let (store, service, app) = setup();
    for i in 0..20 {
        app.put(
            &store,
            A,
            doc(A, i, &format!("apple pie number {i}"), "alice"),
        );
        app.put(
            &store,
            B,
            doc(B, i, &format!("apple tart number {i}"), "bob"),
        );
    }
    let _ = service.index_context(&app, A).await.unwrap();
    let _ = service.index_context(&app, B).await.unwrap();

    for (ctx, other) in [(A, B), (B, A)] {
        for mode in [
            SearchMode::Words,
            SearchMode::Prefix,
            SearchMode::Substring,
            SearchMode::Fuzzy,
        ] {
            let res = service.search(&ctx, &query("apple", mode)).unwrap();
            assert_eq!(res.total, 20, "{mode:?}");
            assert!(res
                .hits
                .iter()
                .all(|h| h.id[0] == ctx[0] && h.id[0] != other[0]));
        }
    }
    // B's own words are not findable from A at all.
    assert_eq!(
        service
            .search(&A, &query("tart", SearchMode::Words))
            .unwrap()
            .total,
        0
    );
    // A filter on another context's facet value finds nothing either.
    let mut bob = query("apple", SearchMode::Words);
    bob.filters.push(SearchFilter::Eq {
        field: "sender".to_owned(),
        value: "bob".to_owned(),
    });
    assert_eq!(service.search(&A, &bob).unwrap().total, 0);
    // A context with no index answers empty rather than failing.
    assert_eq!(
        service
            .search(&[0xC; 32], &query("apple", SearchMode::Words))
            .unwrap()
            .total,
        0
    );
}

#[tokio::test]
async fn scores_in_one_context_ignore_every_other_context() {
    let (store, service, app) = setup();
    for i in 0..50 {
        let text = if i % 5 == 0 {
            "rare apple here"
        } else {
            "common words only"
        };
        app.put(&store, A, doc(A, i, text, "alice"));
    }
    let _ = service.index_context(&app, A).await.unwrap();
    let before = service
        .search(&A, &query("apple", SearchMode::Words))
        .unwrap();

    // Flood B with the same term: were IDF shared, "apple" would stop being rare.
    for i in 0..2_000 {
        app.put(&store, B, doc(B, i, "apple apple apple", "bob"));
    }
    let _ = service.index_context(&app, B).await.unwrap();
    let after = service
        .search(&A, &query("apple", SearchMode::Words))
        .unwrap();

    assert_eq!(before.total, 10);
    let scores = |r: &calimero_primitives::search::SearchResponse| {
        r.hits
            .iter()
            .map(|h| (h.id, h.score.to_bits()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        scores(&before),
        scores(&after),
        "A's ranking moved when B changed"
    );
}

#[tokio::test]
async fn edits_and_deletes_land_and_a_crash_replays_from_the_payload() {
    let (store, service, app) = setup();
    for i in 0..300 {
        app.put(&store, A, doc(A, i, "first draft", "alice"));
    }
    let _ = service.index_context(&app, A).await.unwrap();
    for i in 0..100 {
        app.put(&store, A, doc(A, i, "second draft", "alice"));
    }
    for i in 100..150 {
        app.delete(&store, A, doc(A, i, "", "").id);
    }
    // The first attempt dies in extraction: nothing new commits, nothing trims.
    app.fail_extract.store(1, Ordering::SeqCst);
    assert!(service.index_context(&app, A).await.is_err());
    assert_eq!(
        service
            .search(&A, &query("second", SearchMode::Words))
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        dirty::read_after(&store, &A, 0, usize::MAX).unwrap().len(),
        150
    );

    // A new service over the same store (a restart) replays the backlog.
    drop(service);
    let service = SearchService::new(store.clone(), SearchConfig::default());
    let report = service.index_context(&app, A).await.unwrap();
    assert_eq!(report.rows, 150);
    assert_eq!(report.rebuilt, 0, "a current index is not rebuilt");
    assert_eq!(
        service
            .search(&A, &query("second", SearchMode::Words))
            .unwrap()
            .total,
        100
    );
    assert_eq!(
        service
            .search(&A, &query("first", SearchMode::Words))
            .unwrap()
            .total,
        150
    );
    assert_eq!(
        service
            .get_index(&A, "messages")
            .unwrap()
            .unwrap()
            .num_docs(),
        250
    );
    assert!(dirty::read_after(&store, &A, 0, 1).unwrap().is_empty());

    // Replaying an id that is already indexed changes nothing.
    let mut tx = Transaction::default();
    let _ = dirty::stage(&mut tx, &A, &[doc(A, 0, "", "").id]).unwrap();
    store.apply(&tx).unwrap();
    let _ = service.index_context(&app, A).await.unwrap();
    assert_eq!(
        service
            .get_index(&A, "messages")
            .unwrap()
            .unwrap()
            .num_docs(),
        250
    );
}

#[tokio::test]
async fn a_schema_bump_rebuilds_from_a_scan() {
    let (store, service, app) = setup();
    for i in 0..30 {
        app.put(&store, A, doc(A, i, "stable text", "alice"));
    }
    // The first build is a scan of state; the dirty rows it covers are dropped.
    let report = service.index_context(&app, A).await.unwrap();
    assert_eq!((report.rebuilt, report.rows), (30, 0));
    assert!(dirty::read_after(&store, &A, 0, 1).unwrap().is_empty());

    // Unchanged schema: nothing to rebuild.
    let report = service.index_context(&app, A).await.unwrap();
    assert_eq!(report.rebuilt, 0);
    assert_eq!(
        app.scans.load(Ordering::SeqCst),
        1,
        "a fresh index scans; a current one does not"
    );

    app.version.store(2, Ordering::SeqCst);
    let report = service.index_context(&app, A).await.unwrap();
    assert_eq!(report.rebuilt, 30);
    assert_eq!(
        service
            .search(&A, &query("stable", SearchMode::Words))
            .unwrap()
            .total,
        30
    );
}

#[tokio::test]
async fn deleting_a_context_drops_its_index_and_log() {
    let (store, service, app) = setup();
    app.put(&store, A, doc(A, 1, "gone soon", "alice"));
    app.put(&store, B, doc(B, 1, "stays", "bob"));
    let _ = service.index_context(&app, A).await.unwrap();
    let _ = service.index_context(&app, B).await.unwrap();
    app.put(&store, A, doc(A, 2, "pending", "alice"));

    service.delete_context(&A).unwrap();
    assert_eq!(
        service
            .search(&A, &query("gone", SearchMode::Words))
            .unwrap()
            .total,
        0
    );
    assert!(dirty::read_after(&store, &A, 0, 1).unwrap().is_empty());
    assert_eq!(
        service
            .search(&B, &query("stays", SearchMode::Words))
            .unwrap()
            .total,
        1
    );
}
