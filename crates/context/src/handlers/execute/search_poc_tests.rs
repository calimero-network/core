//! Search PoC end to end through a live `ContextManager`: the real
//! `search-chat` wasm (built by `cargo mero build`), a RocksDB store, the dirty
//! row staged into the execute transaction, the indexer driven through
//! `NodeExtractor` (the app's own views), and queries through the app's
//! `search` view and the `search_query` host function.
//!
//! Needs `apps/search-chat/res/search_chat.wasm` (or `SEARCH_CHAT_WASM`);
//! without it every test here says so and passes vacuously, so a plain
//! `cargo test` does not depend on a wasm build.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_context_client::messages::ExecuteError;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    register_context_in_group, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_search::{dirty, SearchConfig, SearchService};
use calimero_store::config::StoreConfig;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use calimero_store_rocksdb::RocksDB;
use futures_util::io::Cursor;
use serde_json::{json, Value};
use tempfile::TempDir;
use tracing::field::{Field, Visit};
use tracing::span;

use crate::search::NodeExtractor;
use crate::test_support::{actor, enrol, enrol_holder};

fn wasm() -> Option<Vec<u8>> {
    let path = std::env::var("SEARCH_CHAT_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/search-chat/res/search_chat.wasm"
        )
        .to_owned()
    });
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            eprintln!("skipping: {path} not built (cargo mero build --manifest-path apps/search-chat/Cargo.toml)");
            None
        }
    }
}

fn global_runtime() {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a multi-threaded runtime")
    });
    let _ = std::thread::scope(|scope| {
        scope
            .spawn(|| runtime.block_on(async { calimero_utils_actix::init_global_runtime() }))
            .join()
    });
}

pub(super) struct Chat {
    pub harness: actor::Harness,
    pub store: Store,
    pub search: Option<Arc<SearchService>>,
    pub executor: PublicKey,
    pub contexts: Vec<ContextId>,
    _dir: TempDir,
}

impl Chat {
    /// A group with `contexts` contexts of search-chat, each initialized, on a
    /// RocksDB store; search on or off.
    pub(super) async fn new(contexts: u8, search_on: bool) -> Option<Self> {
        let wasm = wasm()?;
        global_runtime();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("db")).expect("utf8");
        let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open rocksdb");
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let search = search_on.then(|| SearchService::new(store.clone(), SearchConfig::default()));
        let harness = match &search {
            Some(search) => actor::over_with_search(store.clone(), Arc::clone(search)).await,
            None => actor::over(store.clone()).await,
        };

        let (blob_id, size) = harness
            .node_client
            .add_blob(Cursor::new(wasm.clone()), Some(wasm.len() as u64), None)
            .await
            .expect("store the wasm");
        let application_id = ApplicationId::from([0x5C; 32]);
        let mut handle = store.handle();
        handle
            .put(
                &key::ApplicationMeta::new(application_id),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(blob_id),
                    size,
                    "file://search_chat.wasm".into(),
                    vec![].into(),
                    key::BlobMeta::new([0; 32].into()),
                    types::PackageInfo {
                        package: "com.calimero.search-chat".into(),
                        version: "0.0.0".into(),
                        signer_id: "did:key:test".into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("install the application");

        let group_id = ContextGroupId::from([0x6B; 32]);
        let admin = PrivateKey::from([0x12; 32]).public_key();
        let admin_account = enrol(&store, &group_id, &admin);
        MetaRepository::new(&store)
            .save(
                &group_id,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id,
                        bytecode_id: *blob_id.digest(),
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1_700_000_000,
                    admin_identity: admin_account,
                    owner_identity: admin_account,
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("group meta");
        MembershipRepository::new(&store)
            .add_member(&group_id, &admin_account, GroupMemberRole::Admin)
            .expect("admin");
        let _key_id = GroupKeyring::new(&store, group_id)
            .store_key(&[0x34; 32])
            .expect("group key");
        let executor_sk = PrivateKey::from([0x23; 32]);
        let executor = executor_sk.public_key();
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &executor, executor_sk.as_bytes())
            .expect("namespace identity");
        let account = enrol_holder(&store, &group_id, &executor);
        MembershipRepository::new(&store)
            .add_member(&group_id, &account, GroupMemberRole::Member)
            .expect("member");

        let mut ids = Vec::new();
        for c in 0..contexts {
            let context_id = ContextId::from([0xD0 + c; 32]);
            handle
                .put(
                    &key::ContextMeta::new(context_id),
                    &types::ContextMeta::new(
                        key::ApplicationMeta::new(application_id),
                        [0x01; 32],
                        Vec::new(),
                        None,
                    ),
                )
                .expect("context meta");
            handle
                .put(
                    &key::ContextConfig::new(context_id),
                    &types::ContextConfig::new(0, 0),
                )
                .expect("context config");
            register_context_in_group(&store, &group_id, &context_id).expect("register");
            handle
                .put(
                    &key::ContextIdentity::new(context_id, executor),
                    &types::ContextIdentity {
                        private_key: Some(*executor_sk.as_bytes()),
                    },
                )
                .expect("membership marker");
            ids.push(context_id);
        }
        let chat = Self {
            harness,
            store,
            search,
            executor,
            contexts: ids,
            _dir: dir,
        };
        for c in chat.contexts.clone() {
            let _ = chat.call(c, "init", json!({})).await.expect("init");
        }
        Some(chat)
    }

    /// Run `method` and return its JSON result.
    pub(super) async fn call(
        &self,
        context: ContextId,
        method: &str,
        args: Value,
    ) -> Result<Value, ExecuteError> {
        let response = self
            .harness
            .context_client
            .execute(
                &context,
                &self.executor,
                method.to_owned(),
                serde_json::to_vec(&args).expect("json"),
                None,
            )
            .await?;
        match response.returns {
            Ok(Some(bytes)) => Ok(serde_json::from_slice(&bytes).expect("json result")),
            Ok(None) => Ok(Value::Null),
            Err(err) => panic!("{method} failed: {err:?}"),
        }
    }

    pub(super) async fn index(&self, context: ContextId) -> calimero_search::IndexReport {
        let extractor = NodeExtractor::new(self.harness.context_client.clone());
        self.search
            .as_ref()
            .expect("search on")
            .index_context(&extractor, *context.as_ref())
            .await
            .expect("index")
    }

    pub(super) async fn search(&self, context: ContextId, query: &str, mode: &str) -> Value {
        self.call(context, "search", json!({ "query": query, "mode": mode }))
            .await
            .expect("search")
    }

    pub(super) fn dirty_rows(&self, context: ContextId) -> Vec<dirty::DirtyRow> {
        dirty::read_after(&self.store, context.as_ref(), 0, usize::MAX).expect("dirty rows")
    }
}

fn texts(page: &Value) -> Vec<String> {
    page["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["message"]["text"].as_str().expect("text").to_owned())
        .collect()
}

#[actix::test]
async fn messages_are_indexed_from_the_dirty_log_and_found_by_the_view() {
    let Some(chat) = Chat::new(1, true).await else {
        return;
    };
    let a = chat.contexts[0];
    // The first pass builds the (empty) index from a scan of state and drops
    // the rows it covers, so what follows goes through the dirty log alone.
    let first = chat.index(a).await;
    assert_eq!((first.rebuilt, first.docs), (0, 0), "{first:?}");
    let before = chat.dirty_rows(a).len();
    assert_eq!(before, 0);
    for (id, text) in [
        ("m1", "the merger closes friday"),
        ("m2", "Let's meet at the Café"),
        ("m3", "lunch friday?"),
    ] {
        let _ = chat
            .call(
                a,
                "post",
                json!({ "id": id, "sender": "alice", "text": text, "ts": 1 }),
            )
            .await
            .expect("post");
    }
    // One dirty row per committed execution, written in its batch.
    let rows = chat.dirty_rows(a);
    assert_eq!(rows.len(), before + 3);
    assert!(rows.iter().all(|r| !r.ids.is_empty()));

    // Not indexed yet: the index lags state until the indexer runs.
    assert_eq!(chat.search(a, "friday", "words").await["total"], 0);

    let report = chat.index(a).await;
    assert_eq!(
        (report.rows, report.docs, report.rebuilt),
        (3, 3, 0),
        "{report:?}"
    );
    assert!(
        chat.dirty_rows(a).is_empty(),
        "the log is trimmed after the commit"
    );

    let page = chat.search(a, "friday", "words").await;
    assert_eq!(page["total"], 2);
    assert!(page["hits"][0]["snippet"]
        .as_str()
        .expect("snippet")
        .contains("<b>friday</b>"));
    assert_eq!(
        texts(&chat.search(a, "cafe", "words").await),
        ["Let's meet at the Café"]
    );
    assert_eq!(chat.search(a, "merg", "prefix").await["total"], 1);
    assert_eq!(chat.search(a, "rger clo", "substring").await["total"], 1);
    assert_eq!(
        chat.search(a, "frxdzy", "fuzzy").await["total"],
        0,
        "two edits away"
    );
    assert_eq!(chat.search(a, "frday", "fuzzy").await["total"], 2);

    // Edit and delete: until the indexer catches up, the view re-reads every
    // hit and drops the deleted one; afterwards the index agrees.
    let _ = chat
        .call(
            a,
            "edit",
            json!({ "id": "m1", "text": "the merger slipped" }),
        )
        .await
        .expect("edit");
    let _ = chat
        .call(a, "delete", json!({ "id": "m3" }))
        .await
        .expect("delete");
    let stale = chat.search(a, "friday", "words").await;
    assert_eq!(stale["stale"], 1, "{stale}");
    assert_eq!(
        texts(&stale),
        ["the merger slipped"],
        "a hit shows current state"
    );

    let _ = chat.index(a).await;
    let page = chat.search(a, "friday", "words").await;
    assert_eq!(page["total"], 0, "{page}");
    assert_eq!(chat.search(a, "slipped", "words").await["total"], 1);
}

#[actix::test]
async fn a_view_only_ever_searches_its_own_context() {
    let Some(chat) = Chat::new(2, true).await else {
        return;
    };
    let (a, b) = (chat.contexts[0], chat.contexts[1]);
    let _ = chat
        .call(
            a,
            "post",
            json!({ "id": "x", "sender": "alice", "text": "apple in a", "ts": 1 }),
        )
        .await
        .expect("post");
    let _ = chat
        .call(
            b,
            "post",
            json!({ "id": "x", "sender": "bob", "text": "apple in b", "ts": 1 }),
        )
        .await
        .expect("post");
    let _ = chat
        .call(
            b,
            "post",
            json!({ "id": "y", "sender": "bob", "text": "banana only in b", "ts": 2 }),
        )
        .await
        .expect("post");
    let _ = chat.index(a).await;
    let _ = chat.index(b).await;

    assert_eq!(
        texts(&chat.search(a, "apple", "words").await),
        ["apple in a"]
    );
    assert_eq!(chat.search(a, "banana", "words").await["total"], 0);
    assert_eq!(chat.search(b, "apple", "words").await["total"], 1);
    assert_eq!(chat.search(b, "banana", "words").await["total"], 1);
}

#[actix::test]
async fn a_peer_delta_marks_its_entities_dirty_too() {
    let Some(chat) = Chat::new(2, true).await else {
        return;
    };
    let (a, b) = (chat.contexts[0], chat.contexts[1]);
    // Author in A, then merge-apply A's artifact into B the way the delta
    // applier does: the same entity ids, arriving as a peer's delta.
    let response = chat
        .harness
        .context_client
        .execute(
            &a,
            &chat.executor,
            "post".to_owned(),
            serde_json::to_vec(
                &json!({ "id": "p", "sender": "carol", "text": "from a peer", "ts": 5 }),
            )
            .expect("json"),
            None,
        )
        .await
        .expect("post");
    let before = chat.dirty_rows(b).len();
    let _ = chat
        .harness
        .context_client
        .apply_remote_delta(&b, &chat.executor, response.artifact, None)
        .await
        .expect("apply");
    let rows = chat.dirty_rows(b);
    assert_eq!(
        rows.len(),
        before + 1,
        "the apply batch carried a dirty row"
    );
    let _ = chat.index(b).await;
    assert_eq!(
        texts(&chat.search(b, "peer", "words").await),
        ["from a peer"]
    );
}

#[actix::test]
async fn a_deleted_message_is_never_returned() {
    let Some(chat) = Chat::new(2, true).await else {
        return;
    };
    let (a, b) = (chat.contexts[0], chat.contexts[1]);
    let _ = chat.index(a).await;
    let _ = chat.index(b).await;
    // Authored in A and replicated to B as a peer delta; then deleted in A and
    // the deletion replicated the same way.
    let post = chat
        .harness
        .context_client
        .execute(
            &a,
            &chat.executor,
            "post".to_owned(),
            serde_json::to_vec(
                &json!({ "id": "d", "sender": "dan", "text": "shred this memo", "ts": 1 }),
            )
            .expect("json"),
            None,
        )
        .await
        .expect("post");
    let _ = chat
        .harness
        .context_client
        .apply_remote_delta(&b, &chat.executor, post.artifact, None)
        .await
        .expect("apply the post");
    let _ = chat.index(a).await;
    let _ = chat.index(b).await;
    for c in [a, b] {
        assert_eq!(
            texts(&chat.search(c, "memo", "words").await),
            ["shred this memo"]
        );
    }

    let delete = chat
        .harness
        .context_client
        .execute(
            &a,
            &chat.executor,
            "delete".to_owned(),
            serde_json::to_vec(&json!({ "id": "d" })).expect("json"),
            None,
        )
        .await
        .expect("delete");
    let _ = chat
        .harness
        .context_client
        .apply_remote_delta(&b, &chat.executor, delete.artifact, None)
        .await
        .expect("apply the delete");

    // Before the indexer runs, the index still matches the entity, and the
    // view's re-read from state drops it: no hit, one stale.
    for c in [a, b] {
        let page = chat.search(c, "memo", "words").await;
        assert_eq!(
            (page["total"].clone(), page["stale"].clone()),
            (json!(1), json!(1)),
            "{page}"
        );
        assert!(texts(&page).is_empty(), "{page}");
    }
    // After it runs, the index has dropped the document too.
    for c in [a, b] {
        let _ = chat.index(c).await;
        let page = chat.search(c, "memo", "words").await;
        assert_eq!(
            (page["total"].clone(), page["stale"].clone()),
            (json!(0), json!(0)),
            "{page}"
        );
    }
    // And a full rebuild from state never brings it back.
    let search = chat.search.clone().expect("search");
    search.delete_context(a.as_ref()).expect("drop the index");
    let rebuilt = chat.index(a).await;
    assert_eq!(rebuilt.rebuilt, 0, "{rebuilt:?}");
    assert_eq!(chat.search(a, "memo", "words").await["total"], 0);
}

#[actix::test]
async fn a_write_can_never_search() {
    let Some(chat) = Chat::new(1, true).await else {
        return;
    };
    let a = chat.contexts[0];
    let _ = chat
        .call(
            a,
            "post",
            json!({ "id": "m", "sender": "a", "text": "hello", "ts": 1 }),
        )
        .await
        .expect("post");
    let _ = chat.index(a).await;
    assert_eq!(chat.search(a, "hello", "words").await["total"], 1);

    // A mutating method is never handed the search handle: its call traps
    // before the post that would follow it, and nothing commits.
    let response = chat
        .harness
        .context_client
        .execute(
            &a,
            &chat.executor,
            "search_in_a_write".to_owned(),
            serde_json::to_vec(&json!({ "query": "hello" })).expect("json"),
            None,
        )
        .await
        .expect("the run itself completes");
    let err = response.returns.expect_err("a write searched");
    assert!(format!("{err:?}").contains("search_query"), "{err:?}");
    assert_eq!(chat.call(a, "count", json!({})).await.expect("count"), 1);
    assert!(
        response.artifact.is_empty(),
        "the trapped write produced a delta"
    );
}

#[actix::test]
async fn without_search_nothing_is_written_and_the_view_is_refused() {
    let Some(chat) = Chat::new(1, false).await else {
        return;
    };
    let a = chat.contexts[0];
    let _ = chat
        .call(
            a,
            "post",
            json!({ "id": "m", "sender": "a", "text": "hello", "ts": 1 }),
        )
        .await
        .expect("post");
    assert!(chat.dirty_rows(a).is_empty());
    // The host has no search handle to give: the guest's call traps.
    let response = chat
        .harness
        .context_client
        .execute(
            &a,
            &chat.executor,
            "search".to_owned(),
            serde_json::to_vec(&json!({ "query": "hello" })).expect("json"),
            None,
        )
        .await
        .expect("the run itself completes");
    let err = response.returns.expect_err("search without a node service");
    assert!(format!("{err:?}").contains("search_query"), "{err:?}");
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.0} µs")
    }
}

/// `(p50, p95, p99)` of `n` sequential runs of `f`.
async fn timed<F, Fut>(n: usize, mut f: F) -> (Duration, Duration, Duration)
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut times = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        f(i).await;
        times.push(t.elapsed());
    }
    times.sort();
    (
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 0.99),
    )
}

/// The gas of the last execution. The runtime reports it only in a `debug!`
/// event (`gas_used`, target `calimero_runtime`), so the benchmark installs
/// [`GasTap`] as the global subscriber and reads it back from there.
static LAST_GAS: AtomicU64 = AtomicU64::new(0);

/// A subscriber that keeps only the runtime's `gas_used` event.
struct GasTap;

impl Visit for GasTap {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "gas_used" {
            LAST_GAS.store(value, Ordering::Relaxed);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for GasTap {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event()
            && metadata.target().starts_with("calimero_runtime")
            && metadata.fields().field("gas_used").is_some()
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut GasTap);
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Run `method` once: its result (or why it failed, e.g. exhausted gas) and
/// the gas it used.
async fn metered(
    chat: &Chat,
    context: ContextId,
    method: &str,
    args: &Value,
) -> (Result<Value, String>, u64) {
    LAST_GAS.store(0, Ordering::Relaxed);
    let response = chat
        .harness
        .context_client
        .execute(
            &context,
            &chat.executor,
            method.to_owned(),
            serde_json::to_vec(args).expect("json"),
            None,
        )
        .await;
    let gas = LAST_GAS.load(Ordering::Relaxed);
    let result = match response {
        Ok(response) => match response.returns {
            Ok(Some(bytes)) => Ok(serde_json::from_slice(&bytes).expect("json result")),
            Ok(None) => Ok(Value::Null),
            Err(err) => Err(format!("{err:?}")),
        },
        Err(err) => Err(format!("{err:?}")),
    };
    (result, gas)
}

fn gas(g: u64) -> String {
    format!("{:.2} M", g as f64 / 1e6)
}

/// The process's peak resident set, from `/proc/self/status` (Linux only).
fn peak_rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
        })
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn corpus_text(i: usize) -> String {
    const WORDS: [&str; 16] = [
        "ka", "ri", "to", "me", "su", "lo", "na", "pe", "vi", "do", "ga", "hu", "ze", "bo", "fi",
        "ly",
    ];
    let mut x = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut words = Vec::new();
    for _ in 0..10 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // Zipf-ish: the square of a uniform draw skews toward low ranks.
        let u = (x % 10_000) as f64 / 10_000.0;
        let rank = ((u * u) * 4096.0) as usize;
        words.push(format!(
            "{}{}{}",
            WORDS[rank % 16],
            WORDS[(rank / 16) % 16],
            WORDS[(rank / 256) % 16]
        ));
    }
    if i % 200 == 7 {
        words.push("needle".to_owned());
    }
    if i % 2_000 == 3 {
        words.push("zebrafish".to_owned());
    }
    words.join(" ")
}

/// The end-to-end numbers for `tools/search-poc/README.md`. Run in release:
/// `SEARCH_POC_N=10000 cargo test --release -p calimero-context --lib search_poc_bench -- --ignored --nocapture`
#[actix::test]
#[ignore = "benchmark; needs the search-chat wasm and a release build"]
async fn search_poc_bench() {
    let n: usize = std::env::var("SEARCH_POC_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    // Taken before any execution, so every runtime callsite registers with it.
    let _ = tracing::subscriber::set_global_default(GasTap);
    let Some(on) = Chat::new(1, true).await else {
        return;
    };
    let Some(off) = Chat::new(1, false).await else {
        return;
    };
    let (a, z) = (on.contexts[0], off.contexts[0]);
    println!("\n## End-to-end (live ContextManager, search-chat wasm, RocksDB), {n} messages\n");

    // Build the (empty) index first, so the seeding below reaches it through
    // the dirty log, the way live writes do.
    let _ = on.index(a).await;

    // Seed through the app, 500 messages per execution.
    let t = Instant::now();
    for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
        let batch: Vec<Value> = chunk
            .iter()
            .map(|i| json!({ "id": format!("m{i:07}"), "sender": format!("user{}", i % 50), "text": corpus_text(*i), "ts": *i as u64 }))
            .collect();
        let _ = on
            .call(a, "post_many", json!({ "messages": batch }))
            .await
            .expect("seed");
        let _ = off
            .call(z, "post_many", json!({ "messages": batch }))
            .await
            .expect("seed");
    }
    println!(
        "Seeded both contexts ({n} messages each, 500 per execution) in {:.1} s.\n",
        t.elapsed().as_secs_f64()
    );
    let rows = on.dirty_rows(a);
    let row_bytes: usize = rows.iter().map(|r| 40 + 4 + 32 * r.ids.len()).sum();
    let ids: usize = rows.iter().map(|r| r.ids.len()).sum();
    println!(
        "Dirty log after seeding: {} rows (one per execution; {:.1} ids, {} B per row on average).\n",
        rows.len(),
        ids as f64 / rows.len() as f64,
        row_bytes / rows.len()
    );

    // Index the backlog: extraction through the app's views + tantivy.
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Incremental, bulk: draining the {n}-message dirty backlog took {:.2} s = {} per message (extract through the wasm view {}, tantivy {}); {} rows, {} ids, {} docs, {} commits.\n",
        total.as_secs_f64(),
        fmt(total / n as u32),
        fmt(report.extract_time / n as u32),
        fmt(report.index_time / n as u32),
        report.rows,
        report.ids,
        report.docs,
        report.commits
    );
    // Full rebuild (first build / snapshot / version bump): drop it, scan it back.
    let search = on.search.clone().expect("search");
    search.delete_context(a.as_ref()).expect("drop the index");
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Full build from a scan of state: {:.2} s = {} per message (scan through the wasm view {}, tantivy {}); {} documents.\n",
        total.as_secs_f64(),
        fmt(total / n as u32),
        fmt(report.extract_time / n as u32),
        fmt(report.index_time / n as u32),
        report.rebuilt
    );

    // Write-path overhead: single-message posts, search on vs off.
    let iters = 200;
    let post = |i: usize| json!({ "id": format!("x{i}"), "sender": "u", "text": corpus_text(i + n), "ts": 1 });
    let (on50, on95, on99) = timed(iters, |i| {
        let (on, args) = (&on, post(i));
        async move {
            let _ = on.call(a, "post", args).await.expect("post");
        }
    })
    .await;
    let (off50, off95, off99) = timed(iters, |i| {
        let (off, args) = (&off, post(i));
        async move {
            let _ = off.call(z, "post", args).await.expect("post");
        }
    })
    .await;
    let (_, on_gas) = metered(&on, a, "post", &post(iters)).await;
    let (_, off_gas) = metered(&off, z, "post", &post(iters)).await;
    let single = on.dirty_rows(a);
    let last = single.last().expect("a row");
    println!(
        "| one `post` execution at {n} messages | p50 | p95 | p99 | gas |\n|---|---|---|---|---|"
    );
    println!(
        "| search off | {} | {} | {} | {} |",
        fmt(off50),
        fmt(off95),
        fmt(off99),
        gas(off_gas)
    );
    println!(
        "| search on (dirty row staged in the batch) | {} | {} | {} | {} |",
        fmt(on50),
        fmt(on95),
        fmt(on99),
        gas(on_gas)
    );
    println!(
        "\nA single post's dirty row names {} entity ids = {} B (key 40 + value {}).\n",
        last.ids.len(),
        40 + 4 + 32 * last.ids.len(),
        4 + 32 * last.ids.len()
    );
    let posts = single.len();
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Incremental, per send: indexing the {posts} single-post rows took {} = {} per post (extract {}, tantivy {}, {} commits).\n",
        fmt(total),
        fmt(total / posts as u32),
        fmt(report.extract_time / posts as u32),
        fmt(report.index_time / posts as u32),
        report.commits
    );

    // Queries through the view (wasm + host fn + re-read of each hit).
    println!(
        "| query through the `search` view | total | p50 | p95 | p99 | gas |\n|---|---|---|---|---|---|"
    );
    let (p50, p95, p99) = timed(50, |_| {
        let on = &on;
        async move {
            let _ = on.call(a, "count", json!({})).await.expect("count");
        }
    })
    .await;
    let (_, count_gas) = metered(&on, a, "count", &json!({})).await;
    println!(
        "| (floor: the `count` view, no search) | — | {} | {} | {} | {} |",
        fmt(p50),
        fmt(p95),
        fmt(p99),
        gas(count_gas)
    );
    for (label, query, mode, sender) in [
        ("rare word", "zebrafish", "words", None),
        ("common word, top-20", "kakaka", "words", None),
        ("no match", "qqxqq", "words", None),
        ("prefix", "needl", "prefix", None),
        ("infix substring", "eedl", "substring", None),
        ("two-term AND", "needle kakaka", "words", None),
        ("sender facet + word", "kakaka", "words", Some("user3")),
        ("fuzzy (distance 1)", "neadle", "fuzzy", None),
    ] {
        let args = json!({ "query": query, "mode": mode, "sender": sender });
        let (result, used) = metered(&on, a, "search", &args).await;
        let total = result.expect("search")["total"].clone();
        let (p50, p95, p99) = timed(50, |_| {
            let (on, args) = (&on, args.clone());
            async move {
                let _ = on.call(a, "search", args).await.expect("search");
            }
        })
        .await;
        println!(
            "| {label} (`{query}`) | {total} | {} | {} | {} | {} |",
            fmt(p50),
            fmt(p95),
            fmt(p99),
            gas(used)
        );
    }
    println!(
        "\n| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |\n|---|---|---|---|---|"
    );
    for (label, term) in [
        ("rare word", "zebrafish"),
        ("common word", "kakaka"),
        ("no match", "qqxqq"),
    ] {
        let args = json!({ "term": term });
        let (result, used) = metered(&on, a, "scan_search", &args).await;
        let (p50, p95, _) = timed(10, |_| {
            let (on, args) = (&on, args.clone());
            async move {
                let _ = metered(on, a, "scan_search", &args).await;
            }
        })
        .await;
        let total = match result {
            Ok(page) => page["total"].to_string(),
            Err(err) if err.contains("exhausted its gas") => "gas exhausted".to_owned(),
            Err(err) => panic!("scan_search failed: {err}"),
        };
        println!(
            "| {label} (`{term}`) | {total} | {} | {} | {} |",
            fmt(p50),
            fmt(p95),
            gas(used)
        );
    }

    // Freshness through the real indexer loop.
    let indexer = tokio::spawn(search.run_indexer(Arc::new(NodeExtractor::new(
        on.harness.context_client.clone(),
    ))));
    let mut lags = Vec::new();
    for i in 0..20 {
        let token = format!("fresh{i}token");
        let _ = on.call(a, "post", json!({ "id": format!("f{i}"), "sender": "u", "text": format!("hello {token}"), "ts": 1 })).await.expect("post");
        let t = Instant::now();
        loop {
            if on.search(a, &token, "words").await["total"] == 1 {
                break;
            }
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "{token} never indexed"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        lags.push(t.elapsed());
        tokio::time::sleep(Duration::from_millis((i * 37 % 250) as u64)).await;
    }
    indexer.abort();
    lags.sort();
    println!(
        "\nFreshness through the live indexer (250 ms commit interval, measured from the post returning): p50 {}, p95 {}, max {}.\n",
        fmt(percentile(&lags, 0.5)),
        fmt(percentile(&lags, 0.95)),
        fmt(*lags.last().expect("lags"))
    );
    println!("Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): {}.\n", peak_rss());
}
