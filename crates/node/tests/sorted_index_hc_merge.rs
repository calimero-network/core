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
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_runtime::{Engine, Module};
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{Metadata, StorageType};
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
    /// The device key backing `executor`.
    ///
    /// Needed only since authored entries entered this harness. A `User` action
    /// is signed by the node's DEVICE key after execution, by a pass that lives
    /// in `calimero-context` (`handlers::execute::signing`) and is `pub(crate)`
    /// there, so a test outside that crate cannot call it. `sign_artifact`
    /// below reproduces the one step of it that matters here, and it needs a
    /// real keypair to do it: before this field `executor` was an arbitrary
    /// 32-byte value with no private key behind it, which is fine for `Public`
    /// entries and impossible for signed ones.
    signing: PrivateKey,
    _dir: TempDir,
}

impl Node {
    /// `seed` is the device PRIVATE key; `executor` is derived from it, so the
    /// two are a real keypair rather than an arbitrary public value.
    fn new(seed: [u8; 32]) -> Self {
        let dir = TempDir::with_prefix("_3333_hc_merge").expect("tempdir");
        let path = dir.path().to_owned().try_into().expect("path conversion");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("open rocksdb");
        let store = Store::new(Arc::new(db));
        let signing = PrivateKey::from(seed);
        Node {
            store,
            executor: signing.public_key(),
            signing,
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
    // The zero account, preserved from when every test here wrote Public
    // entries and the stamp was immaterial. `run_wasm_as` is for the ones where
    // it is not.
    run_wasm_as(node, AccountId::from([0u8; 32]), method, params, commit)
}

/// [`run_wasm`], writing as a named account.
///
/// Needed the moment an authored collection is in play: the owner stamp comes
/// from the account executing the write, so two nodes writing as the same
/// account produce entries that are indistinguishable by owner and make any
/// assertion about provenance vacuous.
fn run_wasm_as(
    node: &Node,
    account: AccountId,
    method: &str,
    params: &Value,
    commit: bool,
) -> (Vec<u8>, Vec<u8>) {
    let (_, module) = engine_module();
    let input = serde_json::to_vec(params).unwrap();
    let mut storage = ContextStorage::from(node.store.clone(), node.ctx());
    let outcome = module
        .run(
            node.ctx(),
            account,
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
    apply_foreign_delta_as(receiver, sender_artifact, incoming_root, None)
}

/// [`apply_foreign_delta`], naming the account the sender's signing key speaks
/// for.
///
/// `ApplyContext::signer_account` is the bridge between "a signature names a
/// KEY" and "authorization names an ACCOUNT". `calimero-storage` deliberately
/// cannot resolve it — that needs the device bindings folded to the action's
/// causal cut — so the node resolves it and passes it in, and `None` is a
/// refusal rather than a default (letting it fall back to the locally executing
/// account would let a remote action authorize itself).
///
/// `Public` entries never consult it, which is why every test here predating
/// authored collections could pass `ApplyContext::empty()`.
fn apply_foreign_delta_as(
    receiver: &Node,
    sender_artifact: &[u8],
    incoming_root: &(Vec<u8>, Metadata),
    signer_account: Option<calimero_account::AccountId>,
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
            let ctx = ApplyContext {
                signer_account,
                ..ApplyContext::empty()
            };
            Interface::<MainStorage>::apply_action(action, &ctx).expect("apply_action");
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

// ---------------------------------------------------------------------------
// AuthoredSortedMap: the authored-and-ordered intersection.
//
// `AuthoredSortedMap` is the only collection that is BOTH per-entry owned
// (`StorageType::User`, signed per action and verified against its owner at
// apply) and backed by a node-local ordered index. The two halves meet exactly
// on this path, and neither half's existing coverage reaches it:
//
//   * the ordered-index tests above (#3333) run over `SortedSet` — Public
//     entries, so nothing is signed and no owner is checked;
//   * the authored-collection tests in `crates/storage` never touch an ordered
//     index, because until `AuthoredSortedMap` no authored collection had one.
//
// What makes the intersection worth its own test: a remote apply mutates
// entries host-side WITHOUT going through `insert`, so it leaves the ordered
// index's validity marker stale and the next ordered read must notice and
// rebuild. A rebuild reads back the entries it is indexing — so if applying a
// foreign delta cost an entry its owner stamp, or if the rebuild collected
// entries it should not have, an ordered read is where it would show.
// ---------------------------------------------------------------------------

/// Sign the `User` actions in an artifact with the writing node's device key.
///
/// Reproduces the single step of `calimero-context`'s post-execution signing
/// pass (`handlers::execute::signing::sign_authorized_actions`, `pub(crate)`
/// there and so unreachable from here) that authored entries depend on.
///
/// Everything else is already in place by the time the artifact exists:
/// `calimero-storage` stamps the nonce and the `signer` device on a local
/// `User` write (see `Interface::save_raw`), leaving only the signature as the
/// `[0; 64]` placeholder. A receiver rejects that placeholder outright — the
/// `InvalidSignature` this harness produced before this function existed.
///
/// Signing alone is not enough to make a remote `User` action apply. The
/// signature names a KEY while authorization names an ACCOUNT, and
/// `user_action_authorized` requires BOTH: the ed25519 check against the named
/// device, and `ApplyContext::signer_account == owner`. A `None` there is a
/// refusal, not a default — see `apply_foreign_delta_as`.
fn sign_artifact(node: &Node, artifact: &[u8]) -> Vec<u8> {
    let sign_actions = |actions: &mut Vec<Action>| {
        for action in actions.iter_mut() {
            // The nonce was set by `calimero-storage` as `metadata.updated_at`
            // (`deleted_at` for a delete). The signing pass RE-STAMPS it onto
            // `sig_data` before hashing, because the nonce carried at
            // outcome-build time can differ from the final `updated_at` — and a
            // payload hashed before that stamp commits to a stale nonce while
            // the action ships the new one, which every receiver then rejects.
            // Stamp first, hash second.
            let (metadata, nonce) = match action {
                Action::Add { metadata, .. } | Action::Update { metadata, .. } => {
                    let nonce = *metadata.updated_at;
                    (metadata, nonce)
                }
                Action::DeleteRef {
                    metadata,
                    deleted_at,
                    ..
                } => {
                    let nonce = *deleted_at;
                    (metadata, nonce)
                }
            };

            let should_sign = match &mut metadata.storage_type {
                StorageType::User {
                    signature_data: Some(sig_data),
                    ..
                } => {
                    let placeholder = sig_data.signature == [0; 64];
                    if placeholder {
                        sig_data.nonce = nonce;
                    }
                    placeholder
                }
                _ => false,
            };
            if !should_sign {
                continue;
            }

            // Payload now reflects the stamped nonce — sign exactly what ships.
            let payload = action.payload_for_signing();
            let signature = node.signing.sign(&payload).expect("sign action");
            let metadata = match action {
                Action::Add { metadata, .. } | Action::Update { metadata, .. } => metadata,
                Action::DeleteRef { metadata, .. } => metadata,
            };
            if let StorageType::User {
                signature_data: Some(sig_data),
                ..
            } = &mut metadata.storage_type
            {
                sig_data.signature = signature.to_bytes();
            }
        }
    };

    let mut delta = from_slice::<StorageDelta>(artifact).expect("decode delta");
    match &mut delta {
        StorageDelta::Actions(actions) => sign_actions(actions),
        StorageDelta::CausalActions { actions, .. } => sign_actions(actions),
    }
    borsh::to_vec(&delta).expect("re-encode delta")
}

/// An ordered, prefix-scoped read on `node`.
fn authored_prefix(node: &Node, prefix: &str) -> Vec<String> {
    let (bytes, _) = run_wasm(
        node,
        "authored_sorted_prefix",
        &json!({ "prefix": prefix }),
        false,
    );
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let v = v.get("output").cloned().unwrap_or(v);
    serde_json::from_value(v).unwrap_or_default()
}

/// The account owning `key` on `node`, as the app reports it.
fn authored_owner(node: &Node, key: &str) -> String {
    let (bytes, _) = run_wasm(
        node,
        "authored_sorted_get_owner",
        &json!({ "key": key }),
        false,
    );
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let v = v.get("output").cloned().unwrap_or(v);
    v.as_str().unwrap_or_default().to_owned()
}

/// Two nodes, two accounts, concurrent authored writes into one prefix.
///
/// Asserts both halves on both nodes after the exchange:
///   1. the ORDERED, prefix-scoped read converges — and stays scoped, so a
///      rebuild that over-collected fails here as loudly as one that
///      under-collected;
///   2. each entry still names the account that wrote it. Ordering is derived
///      per node and never replicated, so it must not cost an entry its
///      provenance on the way across.
#[test]
fn authored_sorted_map_ordered_read_converges_and_keeps_its_owners() {
    let node_a = Node::new([1u8; 32]);
    let node_b = Node::new([2u8; 32]);
    run_wasm(&node_a, "init", &json!({}), true);
    copy_state(&node_a.store, &node_b.store);

    // Each node writes as its OWN account, which is what makes the owner
    // assertions below say anything.
    let (_, artifact_a) = run_wasm_as(
        &node_a,
        node_a.account(),
        "authored_sorted_insert",
        &json!({ "key": "doc/a", "value": "from-a" }),
        true,
    );
    let (_, artifact_b) = run_wasm_as(
        &node_b,
        node_b.account(),
        "authored_sorted_insert",
        &json!({ "key": "doc/b", "value": "from-b" }),
        true,
    );
    // One key outside the prefix, so "stayed scoped" is a real assertion rather
    // than a tautology over a collection that holds nothing else.
    let (_, artifact_z) = run_wasm_as(
        &node_b,
        node_b.account(),
        "authored_sorted_insert",
        &json!({ "key": "other/z", "value": "from-b-elsewhere" }),
        true,
    );

    let root_a = read_root(&node_a);
    let root_b = read_root(&node_b);
    let signed_a = sign_artifact(&node_a, &artifact_a);
    let signed_b = sign_artifact(&node_b, &artifact_b);
    let signed_z = sign_artifact(&node_b, &artifact_z);
    let _ = apply_foreign_delta_as(&node_b, &signed_a, &root_a, Some(node_a.account()));
    let _ = apply_foreign_delta_as(&node_a, &signed_b, &root_b, Some(node_b.account()));
    let _ = apply_foreign_delta_as(&node_a, &signed_z, &root_b, Some(node_b.account()));

    for (label, node) in [("A", &node_a), ("B", &node_b)] {
        assert_eq!(
            authored_prefix(node, "doc/"),
            vec!["doc/a".to_owned(), "doc/b".to_owned()],
            "node {label}: ordered prefix read over authored entries did not converge \
             (or leaked `other/z` into the slice)"
        );
    }

    let account_a = node_a.account().to_string();
    let account_b = node_b.account().to_string();
    assert_ne!(account_a, account_b, "harness: accounts must differ");

    assert_eq!(
        authored_owner(&node_b, "doc/a"),
        account_a,
        "node B: node A's entry lost its owner crossing the apply path"
    );
    assert_eq!(
        authored_owner(&node_a, "doc/b"),
        account_b,
        "node A: node B's entry lost its owner crossing the apply path"
    );
}
