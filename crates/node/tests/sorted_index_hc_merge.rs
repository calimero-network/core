//! Node-level reproduction attempt for calimero-network/core#3333.
//!
//! The two faithful reproductions already on the `fix/3333-sorted-index-convergence`
//! branch (runtime `crdt_conformance` over `__calimero_sync_next`, and
//! `node/primitives` `storage_bridge` over `Interface::apply_action`) both
//! CONVERGE, proving the storage `apply_action` / gossip path self-heals. The
//! issue localizes the surviving defect to the **node-level HashComparison +
//! deferred-root-merge orchestration** — specifically the path where a peer
//! applies a foreign delta's child leaves via `Interface::apply_action` AND
//! merges the app-root entity via the WASM `__calimero_merge_root_state` export
//! (`ContextClient::merge_root_state`) + `Interface::write_pre_merged_root_state`
//! (see `crates/node/src/sync/protocol_selector.rs::dispatch_deferred_root_merges`).
//!
//! Neither `sync_sim` nor `crdt_conformance` drives that root-merge path. This
//! harness does, in-process and without Docker:
//!   * Each node has its OWN real RocksDB-backed `ContextStorage` (temp dir) —
//!     not the storage-crate thread-local index mock and not the runtime
//!     `InMemoryStorage`.
//!   * The REAL compiled `apps/scaffolding-e2e` app drives every WASM call
//!     (`init`, `sorted_tag_add`, `sorted_tags_all`, and crucially
//!     `__calimero_merge_root_state`) via `calimero_runtime::Module::run`.
//!   * Reconciliation replays EXACTLY what `dispatch_deferred_root_merges` does:
//!     non-root leaves through native `Interface::apply_action` (which clears the
//!     `SortedIndexMeta` marker), then the app-root entity through the WASM merge
//!     export + native `write_pre_merged_root_state`.
//!
//! If the ordered `iter()` diverges here, #3333 is reproduced in-process at the
//! layer the issue names, and this becomes the regression test. If it converges,
//! that is decisive evidence the defect is only reachable via the full
//! merobox/real-network path (gossip/HC timing across real merod processes), and
//! the no-Docker route is exhausted.
//!
//! OUTCOME (2026-07-29): the harness CONVERGES. Ordered `iter()` reaches
//! `["a","b"]` on both nodes; membership/len converge too. Two findings:
//!   1. Convergence is carried entirely by the native `apply_action` marker
//!      clear + rebuild-on-read — which self-heals deterministically in a
//!      single process (the branch's storage_bridge repro already showed this).
//!   2. The deferred-root-merge WASM export is a SILENT NO-OP for this Rust
//!      structured root: the stored/wire root doc is `borsh(Entry<AppState>)`
//!      but `merge_root_state_typed` strict-`from_slice`s bare `AppState`, so it
//!      errors `"Not all bytes read"` and `dispatch_deferred_root_merges` skips
//!      it (`continue`). See `deferred_root_merge_is_noop_for_structured_rust_root`.
//!      So the "re-stamp during the deferred merge" hypothesis is refuted here —
//!      that merge never executes its recursive field merge.
//!
//! Conclusion: the #3333 divergence is NOT reachable by this in-process layer; it
//! requires the real merobox/network path (gossip/HC timing across processes).

#![allow(clippy::unwrap_used)]

use calimero_account::AccountId;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use borsh::from_slice;
use calimero_context::handlers::execute::storage::ContextStorage;
use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::{Engine, Module};
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::Metadata;
use calimero_storage::env::with_runtime_env;
use calimero_storage::index::Index;
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::merge::{MergeRootStateRequest, MergeRootStateResponse};
use calimero_storage::store::{Key, MainStorage, StorageAdaptor};
use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use serde_json::{json, Value};
use tempfile::TempDir;

const CTX: [u8; 32] = [7u8; 32];
// The app-root entity id (`Root<T>` entry) that carries the serialised app
// state — the id `merge_root_state_typed::<AppState>` deserialises. Mirrors the
// storage crate's `ROOT_ENTRY_ID` (pub(crate) there); reconstructed here.
const ROOT_ENTRY_ID: [u8; 32] = [118u8; 32];

// ---------------------------------------------------------------------------
// Fixture wasm: build the scaffolding-e2e app once per test-binary run.
// (Mirrors crates/runtime/tests/crdt_conformance.rs.)
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // crates/node/ -> ../../
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn newest_mtime(app_dir: &std::path::Path) -> Option<std::time::SystemTime> {
    fn visit(dir: &std::path::Path, newest: &mut Option<std::time::SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, newest);
            } else if path.extension().is_some_and(|e| e == "rs") {
                if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                    *newest = Some(newest.map_or(m, |cur| cur.max(m)));
                }
            }
        }
    }
    let mut newest = None;
    visit(&app_dir.join("src"), &mut newest);
    for f in ["Cargo.toml", "build.rs"] {
        if let Ok(m) = std::fs::metadata(app_dir.join(f)).and_then(|m| m.modified()) {
            newest = Some(newest.map_or(m, |cur| cur.max(m)));
        }
    }
    newest
}

fn scaffolding_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = workspace_root();
        let app_dir = root.join("apps/scaffolding-e2e");
        let wasm_path = app_dir.join("res/scaffolding_e2e.wasm");

        let wasm_mtime = std::fs::metadata(&wasm_path)
            .and_then(|m| m.modified())
            .ok();
        let newest_src = newest_mtime(&app_dir);
        let needs_build = match (wasm_mtime, newest_src) {
            (Some(w), Some(s)) => w < s,
            _ => true,
        };
        if needs_build {
            let output = Command::new(env!("CARGO"))
                .args([
                    "run",
                    "-q",
                    "-p",
                    "cargo-mero",
                    "--",
                    "mero",
                    "build",
                    "--manifest-path",
                ])
                .arg(app_dir.join("Cargo.toml"))
                .output()
                .expect("failed to spawn cargo mero build");
            assert!(
                output.status.success(),
                "building scaffolding-e2e wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        std::fs::read(&wasm_path).expect("scaffolding_e2e.wasm not found after build")
    })
}

fn engine_module() -> &'static (Engine, Module) {
    static EM: OnceLock<(Engine, Module)> = OnceLock::new();
    EM.get_or_init(|| {
        let engine = Engine::default();
        let module = engine.compile(scaffolding_wasm()).expect("compile wasm");
        (engine, module)
    })
}

// ---------------------------------------------------------------------------
// A node: an independent RocksDB `Store` + a stable executor identity. The
// `TempDir` is retained so the on-disk DB outlives the test body.
// ---------------------------------------------------------------------------

struct Node {
    store: Store,
    executor: PublicKey,
    _dir: TempDir,
}

impl Node {
    fn new(executor: [u8; 32]) -> Self {
        let dir = TempDir::with_prefix("_3333_hc_merge").expect("tempdir");
        let path = dir.path().to_owned().try_into().expect("path conversion");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("open rocksdb");
        let store = Store::new(Arc::new(db));
        Node {
            store,
            executor: PublicKey::from(executor),
            _dir: dir,
        }
    }

    /// This node's account. Distinct from its device (`executor`) so the two stay
    /// distinguishable; nothing in this scenario is writer-set guarded.
    fn account(&self) -> calimero_account::AccountId {
        let mut bytes = *AsRef::<[u8; 32]>::as_ref(&self.executor);
        bytes[1] = 0xAC;
        calimero_account::AccountId::from(bytes)
    }

    fn ctx(&self) -> ContextId {
        ContextId::from(CTX)
    }
}

/// Run a WASM method against a fresh `ContextStorage` over this node's store,
/// committing the temporal writes iff a root hash was produced (mirrors
/// `internal_execute`'s commit rule). Returns the outcome's `(returns_bytes,
/// artifact)`.
fn run_wasm(node: &Node, method: &str, params: &Value, commit: bool) -> (Vec<u8>, Vec<u8>) {
    let (_, module) = engine_module();
    let input = serde_json::to_vec(params).unwrap();
    let mut storage = ContextStorage::from(node.store.clone(), node.ctx());
    let outcome = module
        .run(
            node.ctx(),
            AccountId::from([0u8; 32]),
            node.executor,
            method,
            &input,
            &mut storage,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("{method} trapped: {e}"));
    let artifact = outcome.artifact;
    let returns = match outcome.returns {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => panic!("{method} returned error: {e:?}"),
    };
    if commit {
        // Persist the temporal state writes. The ordered-index writes went
        // straight to RocksDB already (immediate, non-transactional), same as
        // production.
        storage.commit().expect("commit context storage");
    }
    (returns, artifact)
}

/// Copy the synced context state (`Column::State`) from one store to another —
/// used to give both nodes a byte-identical post-`init` base without re-running
/// the (wall-clock-seeded, hence non-deterministic across runs) init on each.
fn copy_state(from: &Store, to: &Store) {
    let hi = vec![0xFFu8; 128];
    let pairs = from
        .raw_scan(Column::State, &[], &hi, None)
        .expect("scan State");
    for (k, v) in pairs {
        to.raw_put(Column::State, &k, &v).expect("put State");
    }
}

/// Read the app-root entity's stored bytes + metadata on `node` (what the
/// deferred-root-merge dispatcher reads as `existing`).
fn read_root(node: &Node) -> (Vec<u8>, Metadata) {
    let env = create_runtime_env(&node.store, node.ctx(), node.executor, node.account());
    with_runtime_env(env, || {
        let id = Id::new(ROOT_ENTRY_ID);
        let meta = Index::<MainStorage>::get_index(id)
            .ok()
            .flatten()
            .map(|idx| idx.metadata)
            .unwrap_or_default();
        let existing =
            <MainStorage as StorageAdaptor>::storage_read(Key::Entry(id)).unwrap_or_default();
        (existing, meta)
    })
}

/// Faithfully replay `dispatch_deferred_root_merges` for a single foreign delta:
/// apply the delta's non-root child leaves via `Interface::apply_action` (the
/// marker-clearing HC leaf path), then merge the app-root entity via the WASM
/// `__calimero_merge_root_state` export + `write_pre_merged_root_state`.
/// Returns `true` if the WASM root-state merge succeeded and was written back,
/// `false` if it errored and was skipped (production's dispatcher `continue`s on
/// a WASM merge error — see `dispatch_deferred_root_merges`).
fn apply_foreign_delta(
    receiver: &Node,
    sender_artifact: &[u8],
    incoming_root: &(Vec<u8>, Metadata),
) -> bool {
    let (_, module) = engine_module();

    // 1. Decode the sender's delta into actions and apply every NON-root leaf
    //    through the native apply path — exactly what HC's DFS does, and what
    //    the branch's storage_bridge repro proved self-heals in isolation.
    let actions = match from_slice::<StorageDelta>(sender_artifact).expect("decode delta") {
        StorageDelta::Actions(a) => a,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    let env = create_runtime_env(
        &receiver.store,
        receiver.ctx(),
        receiver.executor,
        receiver.account(),
    );
    with_runtime_env(env, || {
        for action in actions {
            if calimero_storage::collections::is_app_root_entry(action.id()) {
                // Root entity is deferred to the WASM merge below.
                continue;
            }
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                .expect("apply_action");
        }
    });

    // 2. Build the deferred-root-merge request. `incoming` = the sender's
    //    app-root bytes + its update timestamp captured at write time;
    //    `existing` = the receiver's current app-root bytes + metadata.
    let (incoming, incoming_meta) = incoming_root.clone();
    let (existing, existing_meta) = read_root(receiver);
    let existing_ts: u64 = *existing_meta.updated_at;
    let incoming_ts: u64 = *incoming_meta.updated_at;
    let request = MergeRootStateRequest {
        existing,
        incoming,
        existing_created_at: existing_meta.created_at,
        existing_ts,
        incoming_ts,
    };
    let payload = borsh::to_vec(&request).unwrap();

    // 3. Invoke the REAL WASM merge export. Its temporal writes are NOT
    //    committed (merge is a pure byte->byte function; the dispatcher writes
    //    the result back separately), but any ordered-index side effect it
    //    performs lands immediately in RocksDB — the exact production behaviour.
    let mut merge_storage = ContextStorage::from(receiver.store.clone(), receiver.ctx());
    let outcome = module
        .run(
            receiver.ctx(),
            AccountId::from([0u8; 32]),
            receiver.executor,
            "__calimero_merge_root_state",
            &payload,
            &mut merge_storage,
            None,
            None,
        )
        .expect("merge export run");
    let return_bytes = outcome
        .returns
        .expect("merge returns ok")
        .expect("merge returned bytes");
    drop(merge_storage); // discard temporal (matches dispatcher: no commit here)
    let merged = match from_slice::<MergeRootStateResponse>(&return_bytes).expect("decode resp") {
        MergeRootStateResponse::Ok(bytes) => bytes,
        MergeRootStateResponse::Err(msg) => {
            // Mirror `dispatch_deferred_root_merges`: a WASM merge error is
            // logged and the entry is SKIPPED (`continue`) — never fatal. The
            // next sync tick re-attempts. So the receiver keeps its existing
            // root doc; child leaves already applied above via `apply_action`.
            eprintln!(
                "  [apply_foreign_delta] WASM merge returned Err (skipped, mirrors \
                 dispatch_deferred_root_merges): {msg}"
            );
            return false;
        }
    };

    // 4. Write the merged bytes back via the native pre-merged root-state path.
    let mut new_meta = existing_meta.clone();
    new_meta.updated_at = existing_ts.max(incoming_ts).into();
    let env = create_runtime_env(
        &receiver.store,
        receiver.ctx(),
        receiver.executor,
        receiver.account(),
    );
    with_runtime_env(env, || {
        Interface::<MainStorage>::write_pre_merged_root_state(
            Id::new(ROOT_ENTRY_ID),
            &merged,
            new_meta,
        )
        .expect("write_pre_merged_root_state");
    });
    true
}

/// Query an ordered read on `node` and return the `Vec<String>` result.
fn ordered_tags(node: &Node) -> Vec<String> {
    let (bytes, _) = run_wasm(node, "sorted_tags_all", &json!({}), false);
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let v = v.get("output").cloned().unwrap_or(v);
    serde_json::from_value(v).unwrap_or_default()
}

fn contains_tag(node: &Node, tag: &str) -> bool {
    let (bytes, _) = run_wasm(node, "sorted_tag_contains", &json!({ "tag": tag }), false);
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let v = v.get("output").cloned().unwrap_or(v);
    v.as_bool().unwrap_or(false)
}

/// Documents the decisive finding from this investigation: for a Rust
/// `#[app::state]` (structured) root, the deferred-root-merge WASM export is a
/// **silent no-op**. The app-root doc is stored (and shipped on the HC wire) as
/// `borsh(Entry<AppState>)` — the `AppState` value followed by the `Entry`'s
/// `Element` framing (see `calimero_storage::collections::nested`, entries are
/// `find_by_id::<Entry<T>>`). But `merge_root_state_typed::<AppState>` does a
/// strict `borsh::from_slice::<AppState>`, so it reads the `AppState` prefix and
/// then rejects the trailing `Element` bytes with `"Not all bytes read"`.
/// `dispatch_deferred_root_merges` catches that error and `continue`s (skips),
/// so the recursive field merge (`SortedSet::merge` et al.) never runs on this
/// path — refuting the "re-stamp the ordered-index marker during the deferred
/// root merge" hypothesis for a Rust structured root.
///
/// Fed EXACTLY what production feeds (`find_by_id_raw == storage_read(Entry)`),
/// a no-op merge (existing == incoming == the stored doc, with
/// `created_at != updated_at` so the bootstrap fast-path does not short-circuit
/// the deserialize) MUST therefore return `Err`.
#[test]
fn deferred_root_merge_is_noop_for_structured_rust_root() {
    let node = Node::new([1u8; 32]);
    run_wasm(&node, "init", &json!({}), true);
    run_wasm(&node, "sorted_tag_add", &json!({ "tag": "a" }), true);

    // The serialized app-root doc lives at ROOT_ENTRY_ID, and `find_by_id_raw`
    // (what HC ships as the leaf `incoming`) equals `storage_read` (what the
    // dispatcher reads as `existing`) — so both merge inputs carry the SAME
    // `Entry<T>` framing production feeds.
    let (doc, meta) = read_root(&node);
    assert!(
        !doc.is_empty(),
        "app-root doc must be stored at ROOT_ENTRY_ID"
    );
    assert_ne!(
        meta.created_at, *meta.updated_at,
        "the write advanced updated_at past created_at, so the merge's bootstrap \
         fast-path will NOT short-circuit the deserialize"
    );

    let (_, module) = engine_module();
    let request = MergeRootStateRequest {
        existing: doc.clone(),
        incoming: doc.clone(),
        existing_created_at: meta.created_at,
        existing_ts: *meta.updated_at,
        incoming_ts: *meta.updated_at,
    };
    let payload = borsh::to_vec(&request).unwrap();
    let mut cs = ContextStorage::from(node.store.clone(), node.ctx());
    let outcome = module
        .run(
            node.ctx(),
            AccountId::from([0u8; 32]),
            node.executor,
            "__calimero_merge_root_state",
            &payload,
            &mut cs,
            None,
            None,
        )
        .expect("merge run");
    let ret = outcome.returns.expect("ok").expect("bytes");
    match from_slice::<MergeRootStateResponse>(&ret).expect("decode") {
        MergeRootStateResponse::Ok(_) => panic!(
            "unexpected: the deferred root merge succeeded for a structured Rust root — \
             the Entry<T> framing described in this test's doc-comment must have changed; \
             re-evaluate whether the deferred merge now actually runs the recursive field \
             merge (and thus whether the #3333 re-stamp hypothesis is back in play)"
        ),
        MergeRootStateResponse::Err(e) => {
            assert!(
                e.contains("Not all bytes read"),
                "expected the Entry<T>-framing deserialize error, got: {e}"
            );
        }
    }
}

/// The #3333 reproduction: two nodes concurrently add one distinct SortedSet
/// element each, then each applies the other's delta through the real
/// HashComparison deferred-root-merge path. All reads — membership AND ordered
/// iteration — must converge to the full set on BOTH nodes.
#[test]
fn sorted_set_concurrent_deferred_root_merge_ordered_read() {
    // Leader `init` once, on node A; node B inherits a byte-identical base.
    let node_a = Node::new([1u8; 32]);
    let node_b = Node::new([2u8; 32]);
    run_wasm(&node_a, "init", &json!({}), true);
    copy_state(&node_a.store, &node_b.store);

    // Concurrent, distinct writes.
    let (_, artifact_a) = run_wasm(&node_a, "sorted_tag_add", &json!({ "tag": "a" }), true);
    let (_, artifact_b) = run_wasm(&node_b, "sorted_tag_add", &json!({ "tag": "b" }), true);

    // Capture each node's app-root state at write time (before any
    // reconciliation mutates it) — this is what the peer receives as `incoming`.
    let root_a = read_root(&node_a);
    let root_b = read_root(&node_b);

    // Each node applies the other's delta via the deferred-root-merge path.
    let merged_b = apply_foreign_delta(&node_b, &artifact_a, &root_a);
    let merged_a = apply_foreign_delta(&node_a, &artifact_b, &root_b);
    eprintln!("  root-merge applied? node_b={merged_b} node_a={merged_a}");

    // Membership + count converge (already works today per the issue).
    for (label, node) in [("A", &node_a), ("B", &node_b)] {
        assert!(contains_tag(node, "a"), "node {label} must contain 'a'");
        assert!(contains_tag(node, "b"), "node {label} must contain 'b'");
    }

    // Ordered iteration must ALSO converge — the #3333 assertion.
    let tags_a = ordered_tags(&node_a);
    let tags_b = ordered_tags(&node_b);
    assert_eq!(
        tags_a,
        vec!["a".to_owned(), "b".to_owned()],
        "node A ordered iter() diverged after concurrent deferred-root-merge (core#3333)"
    );
    assert_eq!(
        tags_b,
        vec!["a".to_owned(), "b".to_owned()],
        "node B ordered iter() diverged after concurrent deferred-root-merge (core#3333)"
    );
}
