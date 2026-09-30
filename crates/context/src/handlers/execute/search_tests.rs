//! Full-text search end to end through a live `ContextManager`: the real
//! `search-chat` wasm (built here with `cargo mero build` when it is missing
//! or older than its sources), a RocksDB store, the dirty row staged into the
//! execute transaction, the indexer driven through `NodeContextSource` (the
//! app's generated exports), and queries through the app's `search` view and
//! the `search_query` host function.
//!
//! Every path that changes a context's state is here, each with the check
//! that the index ends up agreeing with state: a local write, a peer's delta
//! (from a node with search off, as an older node is), a snapshot install on a
//! fresh node and over an existing index, a repair sync's direct apply, a
//! delete, and the context's deletion.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

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
use calimero_store::db::Column;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use calimero_store_rocksdb::RocksDB;
use futures_util::io::Cursor;
use serde_json::{json, Value};
use tempfile::TempDir;

use crate::search::NodeContextSource;
use crate::test_support::{actor, enrol, enrol_holder};

mod bench;

/// The indexes search-chat declares. The indexer reports per index: a build,
/// an out-of-band catch and a replayed dirty row each count once per index.
const INDEXES: usize = 3;

/// An app the harness can install.
#[derive(Clone, Copy, Debug)]
pub(super) enum App {
    /// Declares a search index.
    SearchChat,
    /// Declares none: search must cost it nothing.
    KvStore,
}

impl App {
    fn dir(self) -> &'static str {
        match self {
            Self::SearchChat => "search-chat",
            Self::KvStore => "kv-store",
        }
    }

    fn wasm_name(self) -> &'static str {
        match self {
            Self::SearchChat => "search_chat.wasm",
            Self::KvStore => "kv_store.wasm",
        }
    }

    fn wasm(self) -> Vec<u8> {
        static SEARCH_CHAT: OnceLock<Vec<u8>> = OnceLock::new();
        static KV_STORE: OnceLock<Vec<u8>> = OnceLock::new();
        let cell = match self {
            Self::SearchChat => &SEARCH_CHAT,
            Self::KvStore => &KV_STORE,
        };
        cell.get_or_init(|| build_app(self)).clone()
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Newest mtime of the `*.rs` and `Cargo.toml` files under `paths`.
fn newest_mtime(paths: &[PathBuf]) -> Option<std::time::SystemTime> {
    fn visit(path: &Path, newest: &mut Option<std::time::SystemTime>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                visit(&entry.path(), newest);
            }
        } else if path
            .extension()
            .is_some_and(|e| e == "rs" || path.ends_with("Cargo.toml"))
        {
            if let Ok(m) = std::fs::metadata(path).and_then(|m| m.modified()) {
                *newest = Some(newest.map_or(m, |cur| cur.max(m)));
            }
        }
    }
    let mut newest = None;
    for path in paths {
        visit(path, &mut newest);
    }
    newest
}

/// The app's wasm, rebuilt with `cargo mero build` when it is missing or older
/// than the app or the SDK, storage and primitives code it compiles in.
fn build_app(app: App) -> Vec<u8> {
    let root = workspace_root();
    let app_dir = root.join("apps").join(app.dir());
    let wasm = app_dir.join("res").join(app.wasm_name());
    let inputs = [
        app_dir.join("src"),
        app_dir.join("Cargo.toml"),
        root.join("crates/sdk/src"),
        root.join("crates/sdk/macros/src"),
        root.join("crates/storage/src"),
        root.join("crates/primitives/src"),
    ];
    let built = std::fs::metadata(&wasm).and_then(|m| m.modified()).ok();
    let stale = match (built, newest_mtime(&inputs)) {
        (Some(built), Some(source)) => built < source,
        _ => true,
    };
    if stale {
        let output = Command::new(env!("CARGO"))
            .args(["run", "-q", "-p", "cargo-mero", "--", "mero", "build"])
            .arg("--manifest-path")
            .arg(app_dir.join("Cargo.toml"))
            .current_dir(&root)
            .output()
            .expect("spawn cargo mero build");
        assert!(
            output.status.success(),
            "building {} failed:\n{}\n{}",
            app.dir(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::read(&wasm).expect("the app's wasm after its build")
}

fn global_runtime() {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
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

/// One node: a store, a live context manager over it, and `contexts`
/// contexts of one app in one group, each initialized. Two `Chat`s built the
/// same way hold the same context ids, so they stand for two nodes of one
/// context.
pub(super) struct Chat {
    pub harness: actor::Harness,
    pub store: Store,
    pub search: Option<Arc<SearchService>>,
    pub executor: PublicKey,
    pub contexts: Vec<ContextId>,
    app: App,
    _dir: TempDir,
}

impl Chat {
    pub(super) async fn new(contexts: u8, search_on: bool) -> Self {
        Self::of(App::SearchChat, contexts, search_on).await
    }

    pub(super) async fn of(app: App, contexts: u8, search_on: bool) -> Self {
        let wasm = app.wasm();
        global_runtime();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("db")).expect("utf8");
        let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open rocksdb");
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let search = search_on.then(|| SearchService::new(store.clone(), SearchConfig::default()));
        let harness = Self::harness(&store, search.as_ref()).await;

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
                    format!("file://{}", app.wasm_name()).into(),
                    vec![].into(),
                    key::BlobMeta::new([0; 32].into()),
                    types::PackageInfo {
                        package: format!("com.calimero.{}", app.dir()).into(),
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
        let executor_sk = PrivateKey::from([0x23; 32]);
        let executor = executor_sk.public_key();
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &executor, executor_sk.as_bytes())
            .expect("namespace identity");
        let account = enrol_holder(&store, &group_id, &executor);
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
        let members = MembershipRepository::new(&store);
        members
            .add_member(&group_id, &admin_account, GroupMemberRole::Admin)
            .expect("admin");
        // This node administers the group too, so it may delete a context.
        members
            .add_member(&group_id, &account, GroupMemberRole::Admin)
            .expect("member");
        let _key_id = GroupKeyring::new(&store, group_id)
            .store_key(&[0x34; 32])
            .expect("group key");

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
            app,
            _dir: dir,
        };
        for c in chat.contexts.clone() {
            let _ = chat.call(c, "init", json!({})).await.expect("init");
        }
        chat
    }

    async fn harness(store: &Store, search: Option<&Arc<SearchService>>) -> actor::Harness {
        match search {
            Some(search) => actor::over_with_search(store.clone(), Arc::clone(search)).await,
            None => actor::over(store.clone()).await,
        }
    }

    /// Stop this node and start it again over the same store: a new context
    /// manager with every cache cold, and a new search service with nothing
    /// in memory.
    pub(super) async fn restart(mut self) -> Self {
        let search = self
            .search
            .is_some()
            .then(|| SearchService::new(self.store.clone(), SearchConfig::default()));
        self.harness = Self::harness(&self.store, search.as_ref()).await;
        self.search = search;
        // The blob store is the harness's own directory; put the app back.
        let wasm = self.app.wasm();
        let _ = self
            .harness
            .node_client
            .add_blob(Cursor::new(wasm.clone()), Some(wasm.len() as u64), None)
            .await
            .expect("store the wasm again");
        self
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

    /// Run `method` and return the run's own failure, which must be one.
    async fn call_failing(&self, context: ContextId, method: &str, args: Value) -> String {
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
            .await
            .expect("the run itself completes");
        assert!(
            response.artifact.is_empty(),
            "a failed run produced a delta"
        );
        format!("{:?}", response.returns.expect_err("the run failed"))
    }

    /// Post, returning the delta the post produced.
    pub(super) async fn post(&self, context: ContextId, id: &str, text: &str) -> Vec<u8> {
        self.harness
            .context_client
            .execute(
                &context,
                &self.executor,
                "post".to_owned(),
                serde_json::to_vec(&json!({ "id": id, "sender": "alice", "text": text, "ts": 1 }))
                    .expect("json"),
                None,
            )
            .await
            .expect("post")
            .artifact
    }

    pub(super) fn service(&self) -> &Arc<SearchService> {
        self.search.as_ref().expect("search on")
    }

    pub(super) async fn index(&self, context: ContextId) -> calimero_search::IndexReport {
        self.service()
            .index_context(
                &NodeContextSource::new(self.harness.context_client.clone()),
                *context.as_ref(),
            )
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

    /// Every search row (index file chunks and dirty log) of `context`.
    fn search_rows(&self, context: ContextId) -> usize {
        let lo = context.as_ref().to_vec();
        let hi = calimero_primitives::utils::prefix_upper_bound(&lo);
        [Column::SearchIndex, Column::SearchDirty]
            .into_iter()
            .map(|column| {
                self.store
                    .raw_scan(column, &lo, &hi, None)
                    .expect("scan")
                    .len()
            })
            .sum()
    }
}

fn texts(page: &Value) -> Vec<String> {
    let mut out: Vec<String> = page["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["message"]["text"].as_str().expect("text").to_owned())
        .collect();
    out.sort();
    out
}

/// What a snapshot install does to `target`: every state row of `context`
/// written as-is, the way `snapshot.rs` `handle.put`s verified records, and
/// the context's root set to the one the snapshot promised.
fn install_snapshot(source: &Chat, target: &Chat, context: ContextId) {
    let lo = context.as_ref().to_vec();
    let hi = calimero_primitives::utils::prefix_upper_bound(&lo);
    for (k, v) in source
        .store
        .raw_scan(Column::State, &lo, &hi, None)
        .expect("scan the source's state")
    {
        target
            .store
            .raw_put(Column::State, &k, &v)
            .expect("install a state row");
    }
    let root = target
        .harness
        .context_client
        .compute_root_hash(&context)
        .expect("installed root");
    assert_eq!(
        root,
        source
            .harness
            .context_client
            .compute_root_hash(&context)
            .expect("source root"),
        "the install reproduced the source's state"
    );
    target
        .harness
        .context_client
        .force_root_hash(&context, root.into())
        .expect("adopt the snapshot's root");
}

#[actix::test]
async fn messages_are_indexed_from_the_dirty_log_and_found_by_the_view() {
    let chat = Chat::new(1, true).await;
    let a = chat.contexts[0];
    // The first pass builds the (empty) index from a scan of state; what
    // follows goes through the dirty log alone.
    let first = chat.index(a).await;
    assert_eq!((first.builds, first.docs), (INDEXES, 0), "{first:?}");
    assert!(chat.dirty_rows(a).is_empty());
    for (id, text) in [
        ("m1", "the merger closes friday"),
        ("m2", "Let's meet at the Café"),
        ("m3", "lunch friday?"),
    ] {
        let _ = chat.post(a, id, text).await;
    }
    // One dirty row per committed execution, written in its batch, chained
    // by state root.
    let rows = chat.dirty_rows(a);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| !r.ids.is_empty()));
    assert!(rows.windows(2).all(|w| w[0].after == w[1].before));
    assert_eq!(
        rows[2].after,
        chat.harness
            .context_client
            .compute_root_hash(&a)
            .expect("root"),
        "the last row's root is the state's"
    );

    // Not indexed yet: the index lags state until the indexer runs.
    assert_eq!(chat.search(a, "friday", "words").await["total"], 0);

    let report = chat.index(a).await;
    assert_eq!(
        (report.rows, report.docs, report.builds, report.out_of_band),
        (3 * INDEXES, 3, 0, 0),
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
    assert_eq!(chat.search(a, "frxdzy", "fuzzy").await["total"], 0);
    assert_eq!(chat.search(a, "frday", "fuzzy").await["total"], 2);
    // Filters: a sender, and a timestamp range, with and without text.
    let page = chat
        .call(
            a,
            "search",
            json!({ "query": "", "sender": "alice", "since": 1 }),
        )
        .await
        .expect("search");
    assert_eq!(page["total"], 3, "{page}");
    let page = chat
        .call(a, "search", json!({ "query": "friday", "since": 2 }))
        .await
        .expect("search");
    assert_eq!(page["total"], 0, "{page}");

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

    let report = chat.index(a).await;
    assert_eq!((report.builds, report.out_of_band), (0, 0), "{report:?}");
    assert_eq!(chat.search(a, "friday", "words").await["total"], 0);
    assert_eq!(chat.search(a, "slipped", "words").await["total"], 1);
}

#[actix::test]
async fn a_view_only_ever_searches_its_own_context() {
    let chat = Chat::new(2, true).await;
    let (a, b) = (chat.contexts[0], chat.contexts[1]);
    let _ = chat.post(a, "x", "apple in a").await;
    let _ = chat.post(b, "x", "apple in b").await;
    let _ = chat.post(b, "y", "banana only in b").await;
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

/// Backward compatibility: a node without search writes the same deltas an
/// older node does (search adds nothing to a delta, a snapshot or the root
/// hash). Its delta, merge-applied on a node with search, gets a dirty row in
/// the apply's own batch and is found.
#[actix::test]
async fn a_delta_from_a_node_without_search_is_indexed() {
    let old = Chat::new(1, false).await;
    let new = Chat::new(1, true).await;
    let ctx = old.contexts[0];
    let _ = new.index(ctx).await;

    let delta = old.post(ctx, "p", "from a peer without search").await;
    assert!(old.dirty_rows(ctx).is_empty());
    let _ = new
        .harness
        .context_client
        .apply_remote_delta(&ctx, &new.executor, delta, None)
        .await
        .expect("apply");
    assert_eq!(
        new.dirty_rows(ctx).len(),
        1,
        "the apply batch carried a row"
    );
    let report = new.index(ctx).await;
    assert_eq!((report.builds, report.out_of_band), (0, 0), "{report:?}");
    assert_eq!(
        texts(&new.search(ctx, "peer", "words").await),
        ["from a peer without search"]
    );
}

#[actix::test]
async fn a_deleted_message_is_never_returned() {
    let author = Chat::new(1, true).await;
    let peer = Chat::new(1, true).await;
    let c = author.contexts[0];
    let _ = author.index(c).await;
    let _ = peer.index(c).await;
    // Authored on one node and replicated to the other as a peer delta; then
    // deleted and the deletion replicated the same way.
    let post = author.post(c, "d", "shred this memo").await;
    let _ = peer
        .harness
        .context_client
        .apply_remote_delta(&c, &peer.executor, post, None)
        .await
        .expect("apply the post");
    for node in [&author, &peer] {
        let _ = node.index(c).await;
        assert_eq!(
            texts(&node.search(c, "memo", "words").await),
            ["shred this memo"]
        );
    }

    let delete = author
        .harness
        .context_client
        .execute(
            &c,
            &author.executor,
            "delete".to_owned(),
            serde_json::to_vec(&json!({ "id": "d" })).expect("json"),
            None,
        )
        .await
        .expect("delete");
    let _ = peer
        .harness
        .context_client
        .apply_remote_delta(&c, &peer.executor, delete.artifact, None)
        .await
        .expect("apply the delete");

    // Before the indexer runs, the index still matches the entity, and the
    // view's re-read from state drops it: no hit, one stale.
    for node in [&author, &peer] {
        let page = node.search(c, "memo", "words").await;
        assert_eq!(
            (page["total"].clone(), page["stale"].clone()),
            (json!(1), json!(1)),
            "{page}"
        );
        assert!(texts(&page).is_empty(), "{page}");
    }
    // After it runs, the index has dropped the document too.
    for node in [&author, &peer] {
        let _ = node.index(c).await;
        let page = node.search(c, "memo", "words").await;
        assert_eq!(
            (page["total"].clone(), page["stale"].clone()),
            (json!(0), json!(0)),
            "{page}"
        );
    }
}

/// A node that joins by snapshot writes the context's state straight into
/// its store, with no execution and so no dirty row. The first search view
/// tells the indexer the root it saw; the pass that follows finds no index
/// and builds one from state.
#[actix::test]
async fn a_context_joined_by_snapshot_is_built_on_first_use() {
    let source = Chat::new(1, false).await;
    let joiner = Chat::new(1, true).await;
    let c = source.contexts[0];
    for i in 0..5 {
        let _ = source
            .post(c, &format!("m{i}"), &format!("history {i}"))
            .await;
    }
    install_snapshot(&source, &joiner, c);

    // The first view sees nothing yet: the index does not exist.
    assert_eq!(joiner.search(c, "history", "words").await["total"], 0);
    let report = joiner.index(c).await;
    assert_eq!((report.builds, report.rebuilt), (INDEXES, 5), "{report:?}");
    assert_eq!(joiner.search(c, "history", "words").await["total"], 5);
}

/// A snapshot (or any sync that installs state without an execution) over a
/// context whose index is current: the index's root no longer is the
/// state's, and the next pass rebuilds it.
#[actix::test]
async fn a_snapshot_over_an_existing_index_rebuilds_it() {
    let source = Chat::new(1, false).await;
    let node = Chat::new(1, true).await;
    let c = source.contexts[0];
    let _ = node.index(c).await;
    let _ = source.post(c, "s", "arrived in a snapshot").await;
    install_snapshot(&source, &node, c);

    let report = node.index(c).await;
    assert_eq!(
        (report.out_of_band, report.builds, report.rebuilt),
        (INDEXES, INDEXES, 1),
        "{report:?}"
    );
    assert_eq!(
        texts(&node.search(c, "snapshot", "words").await),
        ["arrived in a snapshot"]
    );
    // And the next write chains from the installed root again.
    let _ = node.post(c, "t", "written after the snapshot").await;
    let report = node.index(c).await;
    assert_eq!(
        (report.out_of_band, report.builds, report.rows),
        (0, 0, INDEXES),
        "{report:?}"
    );
}

/// HashComparison and level-wise repair merge a peer's entities through
/// `Interface::apply_action` under the context lock, outside any execution:
/// no dirty row. The broken root chain brings a rebuild.
#[actix::test]
async fn a_repair_sync_apply_is_caught_by_the_root_chain() {
    use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
    use calimero_storage::delta::StorageDelta;
    use calimero_storage::env::with_runtime_env;
    use calimero_storage::interface::{ApplyContext, Interface};
    use calimero_storage::store::MainStorage;

    let peer = Chat::new(1, false).await;
    let node = Chat::new(1, true).await;
    let c = peer.contexts[0];
    let _ = node.index(c).await;

    let delta = peer.post(c, "r", "repaired from a peer").await;
    let Ok(StorageDelta::Actions(actions)) = borsh::from_slice::<StorageDelta>(&delta) else {
        panic!("a local write's artifact is a plain action list");
    };
    let account = calimero_governance_store::account_for_context(&node.store, &c)
        .expect("this node's account");
    let env = create_runtime_env(&node.store, c, node.executor, account);
    let _guard = node.harness.context_client.acquire_lock(&c).await;
    with_runtime_env(env, || {
        for action in actions {
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                .expect("apply_action");
        }
    });
    drop(_guard);
    assert!(node.dirty_rows(c).is_empty(), "a repair writes no row");

    // A local write after the repair: its row chains from the repaired
    // root, which the index never reached.
    let _ = node.post(c, "l", "local after the repair").await;
    let report = node.index(c).await;
    assert_eq!(
        (report.out_of_band, report.builds),
        (INDEXES, INDEXES),
        "{report:?}"
    );
    assert_eq!(
        texts(&node.search(c, "repair", "prefix").await),
        ["local after the repair", "repaired from a peer"]
    );
}

/// The first call on a context after a node restart used to take the write
/// path: the read-only method set is cached per module, and the module is not
/// loaded yet, so the lock selection answered "write" and the run got no
/// search handle. A search view trapped with `SearchUnavailable` exactly once
/// per context per restart.
#[actix::test]
async fn the_first_call_after_a_restart_can_search() {
    let chat = Chat::new(1, true).await;
    let a = chat.contexts[0];
    let _ = chat.post(a, "m", "kept across the restart").await;
    let _ = chat.index(a).await;

    let chat = chat.restart().await;
    let page = chat.search(a, "restart", "words").await;
    assert_eq!(
        texts(&page),
        ["kept across the restart"],
        "the index is in the store and answers at once: {page}"
    );
    // The restart also found the root it left, so nothing was rebuilt.
    let report = chat.index(a).await;
    assert_eq!((report.builds, report.out_of_band), (0, 0), "{report:?}");
}

/// The dirty-log seq survives a restart in the store, not in a clock: rows
/// written after it number above everything the index committed before it.
#[actix::test]
async fn rows_written_after_a_restart_are_indexed() {
    let chat = Chat::new(1, true).await;
    let a = chat.contexts[0];
    let _ = chat.post(a, "b", "before the restart").await;
    let _ = chat.index(a).await;
    let committed = dirty::head(&chat.store, a.as_ref()).expect("head");

    let chat = chat.restart().await;
    let _ = chat.post(a, "c", "after the restart").await;
    let rows = chat.dirty_rows(a);
    assert_eq!(
        rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [committed + 1]
    );
    let report = chat.index(a).await;
    assert_eq!((report.rows, report.builds), (INDEXES, 0), "{report:?}");
    assert_eq!(
        texts(&chat.search(a, "restart", "words").await),
        ["after the restart", "before the restart"]
    );
}

#[actix::test]
async fn deleting_a_context_drops_its_index_and_log() {
    let chat = Chat::new(2, true).await;
    let (a, b) = (chat.contexts[0], chat.contexts[1]);
    let _ = chat.post(a, "x", "gone with the context").await;
    let _ = chat.post(b, "x", "stays with b").await;
    let _ = chat.index(a).await;
    let _ = chat.index(b).await;
    let _ = chat.post(a, "y", "pending when deleted").await;
    assert!(chat.search_rows(a) > 0);

    let response = chat
        .harness
        .context_client
        .delete_context(&a)
        .await
        .expect("delete the context");
    assert!(response.deleted);
    assert_eq!(chat.search_rows(a), 0, "index files and dirty log are gone");
    assert!(chat
        .service()
        .get_index(a.as_ref(), "messages")
        .expect("lookup")
        .is_none());
    assert_eq!(chat.search(b, "stays", "words").await["total"], 1);
}

#[actix::test]
async fn a_write_can_never_search() {
    let chat = Chat::new(1, true).await;
    let a = chat.contexts[0];
    let _ = chat.post(a, "m", "hello").await;
    let _ = chat.index(a).await;
    assert_eq!(chat.search(a, "hello", "words").await["total"], 1);

    // A mutating method is never handed the search handle: its call traps
    // before the post that would follow it, and nothing commits.
    let err = chat
        .call_failing(a, "search_in_a_write", json!({ "query": "hello" }))
        .await;
    assert!(err.contains("search_query"), "{err}");
    assert_eq!(chat.call(a, "count", json!({})).await.expect("count"), 1);
}

#[actix::test]
async fn without_search_nothing_is_written_and_the_view_is_refused() {
    let chat = Chat::new(1, false).await;
    let a = chat.contexts[0];
    let _ = chat.post(a, "m", "hello").await;
    assert_eq!(chat.search_rows(a), 0);
    // The host has no search handle to give: the guest's call traps.
    let err = chat
        .call_failing(a, "search", json!({ "query": "hello" }))
        .await;
    assert!(err.contains("search_query"), "{err}");
}

/// Search is opt-in per app: on a node with search on, an app that declares
/// no index writes no row, opens no index, and wakes no indexer.
#[actix::test]
async fn an_app_without_an_index_pays_nothing() {
    let kv = Chat::of(App::KvStore, 1, true).await;
    let a = kv.contexts[0];
    let _ = kv
        .call(a, "set", json!({ "key": "k", "value": "v" }))
        .await
        .expect("set");
    let got = kv.call(a, "get", json!({ "key": "k" })).await.expect("get");
    assert_eq!(got, json!("v"));
    assert_eq!(kv.search_rows(a), 0, "no dirty row, no index file");
    assert_eq!(kv.service().open_indexes(), 0);
}

/// The shapes a real chat or docs app has, on the real wasm: one index over
/// an `AuthoredVector` and a `Moderated<SortedMap>`, ranked newest first; text
/// indexed through a function of the field (markup left out); a value the
/// app hides dropping out of the index; and a document whose `FugueText`
/// title is edited in place.
#[actix::test]
async fn an_index_over_two_owned_collections_ranks_newest_first_and_follows_nested_text() {
    let chat = Chat::new(1, true).await;
    let a = chat.contexts[0];
    let reply = |html: &'static str, ts: u64| {
        let chat = &chat;
        async move {
            chat.call(a, "reply", json!({ "html": html, "ts": ts }))
                .await
                .expect("reply")
        }
    };
    let old = reply("<p>release <b>notes</b></p>", 10).await;
    let _ = reply("<p>release party</p>", 30).await;
    let hidden = reply("<p>release blockers</p>", 40).await;
    let _ = chat
        .call(
            a,
            "pin",
            json!({ "key": "k", "html": "<i>release</i> checklist", "ts": 20 }),
        )
        .await
        .expect("pin");
    let _ = chat
        .call(a, "hide_reply", json!({ "id": hidden, "hidden": true }))
        .await
        .expect("hide");
    let _ = chat
        .call(
            a,
            "create_doc",
            json!({ "id": "d1", "title": "Quarterly plan" }),
        )
        .await
        .expect("doc");
    let _ = chat.index(a).await;

    let found = |page: &Value| -> Vec<String> {
        page["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .map(|h| h["id"].as_str().expect("id").to_owned())
            .collect()
    };
    let page = chat
        .call(a, "search_replies", json!({ "query": "release" }))
        .await
        .expect("search");
    let stamps: Vec<u64> = page["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["ts"].as_u64().expect("ts"))
        .collect();
    assert_eq!(
        stamps,
        [30, 20, 10],
        "both collections, newest first, the hidden one left out: {page}"
    );
    let ids = found(&page);
    assert_eq!(ids[1], "pinned:k");
    assert_eq!(ids[2], format!("reply:{}", old.as_str().expect("id")));
    let markup = chat
        .call(a, "search_replies", json!({ "query": "b" }))
        .await
        .expect("search");
    assert_eq!(found(&markup).len(), 0, "tags are not text: {markup}");
    let paged = chat
        .call(
            a,
            "search_replies",
            json!({ "query": "release", "limit": 2 }),
        )
        .await
        .expect("search");
    let rest = chat
        .call(
            a,
            "search_replies",
            json!({ "query": "release", "cursor": paged["next_cursor"] }),
        )
        .await
        .expect("search");
    assert_eq!(
        [found(&paged), found(&rest)].concat(),
        found(&page),
        "a second page continues the order"
    );

    // Showing the reply again puts it back, newest.
    let _ = chat
        .call(a, "hide_reply", json!({ "id": hidden, "hidden": false }))
        .await
        .expect("show");
    let _ = chat.index(a).await;
    let page = chat
        .call(a, "search_replies", json!({ "query": "release" }))
        .await
        .expect("search");
    assert_eq!(page["hits"][0]["ts"], 40, "{page}");

    // The title is its own collection's rows: editing it re-indexes the doc.
    assert_eq!(
        chat.call(a, "search_docs", json!({ "query": "plan" }))
            .await
            .expect("search"),
        json!(["d1"])
    );
    let _ = chat
        .call(
            a,
            "append_to_title",
            json!({ "id": "d1", "text": " and budget" }),
        )
        .await
        .expect("edit");
    let _ = chat.index(a).await;
    assert_eq!(
        chat.call(a, "search_docs", json!({ "query": "budget" }))
            .await
            .expect("search"),
        json!(["d1"])
    );
}
