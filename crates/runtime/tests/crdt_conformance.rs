//! CRDT convergence conformance tests that drive a REAL compiled app
//! (`apps/scaffolding-e2e`) through the actual WASM execute path
//! (`calimero_runtime::Module::run`) and assert that concurrent writes on
//! independent nodes converge to the SAME Merkle root after delta sync.
//!
//! Why this exists: the storage crate's unit tests exercise `merge()` and the
//! delta machinery in isolation, but nothing drove a genuinely-compiled app's
//! methods end to end across simulated nodes. These tests close that gap for
//! every non-`Shared` CRDT the scaffolding app exposes.
//!
//! Methodology (mirrors how real nodes work):
//!   * One leader runs `init` natively; peers receive a full-state SNAPSHOT
//!     (modeled as a byte-exact `InMemoryStorage` clone), so all nodes start
//!     from an identical base + metadata.
//!   * Each node then performs writes locally (its own executor id) and
//!     broadcasts the resulting `outcome.artifact` (a `StorageDelta`); peers
//!     apply it via the app's generated `__calimero_sync_next` export.
//!   * Convergence = every node ends on the same root hash AND the same
//!     queried value, regardless of the order deltas were applied.
//!
//! Scope: `Shared` / `Authored` / `User` storage require the node's signing
//! identity (delta apply verifies signatures), which the bare runtime does not
//! provide — those are intentionally out of scope here and belong in a
//! node-layer harness.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use calimero_runtime::logic::Outcome;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{Engine, Module};
use serde_json::{json, to_vec as to_json_vec, Value};

const CTX: [u8; 32] = [7u8; 32];

// ---------------------------------------------------------------------------
// Fixture wasm: build the scaffolding-e2e app once per test-binary run.
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // crates/runtime/ -> ../../
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Newest modification time across the app's build inputs: every `*.rs` under
/// `src/` (recursively), plus `Cargo.toml` and `build.sh`. Returns `None` if
/// nothing could be stat'd (forces a rebuild).
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
    for f in ["Cargo.toml", "build.sh"] {
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

        // (Re)build if the wasm is missing or older than ANY source input
        // (every `*.rs` under `src/`, plus `Cargo.toml` and `build.sh`) — not
        // just `src/lib.rs`, so an edit to a sibling module or the manifest
        // doesn't leave a stale binary in use.
        let wasm_mtime = std::fs::metadata(&wasm_path)
            .and_then(|m| m.modified())
            .ok();
        let newest_src = newest_mtime(&app_dir);
        let needs_build = match (wasm_mtime, newest_src) {
            (Some(w), Some(s)) => w < s,
            _ => true,
        };
        if needs_build {
            // Build via the app's own `build.sh` (handles the wasm32 target +
            // wasm-opt + copy to res/). The path is the compile-time-constant
            // `CARGO_MANIFEST_DIR`, not attacker-controlled. Capture output so a
            // build failure surfaces the actual compiler error, not just a
            // generic assert.
            let output = Command::new("bash")
                .arg(app_dir.join("build.sh"))
                .output()
                .expect("failed to spawn build.sh — is bash on PATH?");
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

// Compiled once and shared across tests. Safe to run concurrently: `Module` is
// `Arc`-backed (its doc-comment notes cloning shares the compiled artifact), and
// every `Module::run` creates its OWN `wasmer::Store` and operates only on the
// caller-supplied `&mut Storage` — there is no shared mutable VM state. These
// tests also keep all storage local to each `Node` (no global/thread-local
// store), so the default parallel `cargo test` execution is race-free.
fn engine_module() -> &'static (Engine, Module) {
    static EM: OnceLock<(Engine, Module)> = OnceLock::new();
    EM.get_or_init(|| {
        let engine = Engine::default();
        let module = engine.compile(scaffolding_wasm()).expect("compile wasm");
        (engine, module)
    })
}

// ---------------------------------------------------------------------------
// A simulated cluster of nodes sharing one compiled module.
// ---------------------------------------------------------------------------

struct Node {
    store: InMemoryStorage,
    executor: [u8; 32],
}

struct Cluster {
    module: &'static Module,
    nodes: Vec<Node>,
}

impl Cluster {
    /// Build an `n`-node cluster: a dedicated leader runs `init`, then every
    /// node receives a byte-exact snapshot (clone) of that post-init storage.
    ///
    /// The leader uses a DISTINCT executor id (`[0xEE; 32]`) so it does not
    /// alias any cluster node; nodes get `[1; 32]`, `[2; 32]`, … This keeps the
    /// init writes attributable to a separate identity from node 0's writes,
    /// matching a real deployment where the context creator and the writers are
    /// distinct, and avoids HLC/executor-seeded artifacts in node 0.
    fn new(n: usize) -> Self {
        // Node executors are `[1; 32]..=[n; 32]` (leader is `[0xEE; 32]`), so a
        // single byte must encode `n` distinct ids. Tests use tiny clusters;
        // assert rather than silently wrap on the `i as u8 + 1` cast.
        assert!(
            (1..=255).contains(&n),
            "Cluster::new supports 1..=255 nodes (one-byte executor ids), got {n}"
        );
        let (_, module) = engine_module();
        let mut leader = Node {
            store: InMemoryStorage::default(),
            executor: [0xEEu8; 32],
        };
        run(module, &mut leader, "init", json!({})).expect("init");

        let mut nodes = Vec::with_capacity(n);
        for i in 0..n {
            nodes.push(Node {
                store: leader.store.clone(),
                executor: [(i as u8) + 1; 32],
            });
        }
        Cluster { module, nodes }
    }

    /// Run a mutating method on `node`, returning the broadcast artifact.
    fn call(&mut self, node: usize, method: &str, params: Value) -> Vec<u8> {
        let m = self.module;
        run(m, &mut self.nodes[node], method, params)
            .unwrap_or_else(|e| panic!("{method} on node {node} failed: {e}"))
            .artifact
    }

    /// Run a mutating method on `node`, returning `(writer_root_hash, artifact)`.
    fn call_full(&mut self, node: usize, method: &str, params: Value) -> ([u8; 32], Vec<u8>) {
        let m = self.module;
        let o = run(m, &mut self.nodes[node], method, params)
            .unwrap_or_else(|e| panic!("{method} on node {node} failed: {e}"));
        (o.root_hash.unwrap_or([0; 32]), o.artifact)
    }

    /// Apply a delta artifact to `node` via `__calimero_sync_next`.
    fn apply(&mut self, node: usize, artifact: &[u8]) {
        let m = self.module;
        apply_delta(m, &mut self.nodes[node], artifact)
            .unwrap_or_else(|e| panic!("apply on node {node} failed: {e}"));
    }

    /// Query a read-only method on `node` and return its JSON value.
    ///
    /// A non-JSON / empty return yields `Value::Null`; callers compare the
    /// result against an expected JSON value, so a deserialize miss surfaces as
    /// a clear assertion mismatch (Null vs expected) rather than being silently
    /// swallowed — adequate for a test helper.
    fn query(&mut self, node: usize, method: &str, params: Value) -> Value {
        let m = self.module;
        let outcome =
            run(m, &mut self.nodes[node], method, params).expect("query method should not trap");
        let bytes = outcome.returns.ok().flatten().unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }
}

fn run(module: &Module, node: &mut Node, method: &str, params: Value) -> eyre::Result<Outcome> {
    let input = to_json_vec(&params)?;
    let outcome = module.run(
        CTX.into(),
        node.executor.into(),
        method,
        &input,
        &mut node.store,
        None,
        None,
    )?;
    if let Err(e) = &outcome.returns {
        eyre::bail!("returns error: {e:?}");
    }
    Ok(outcome)
}

fn apply_delta(module: &Module, node: &mut Node, artifact: &[u8]) -> eyre::Result<[u8; 32]> {
    let outcome = module.run(
        CTX.into(),
        node.executor.into(),
        "__calimero_sync_next",
        artifact,
        &mut node.store,
        None,
        None,
    )?;
    if let Err(e) = &outcome.returns {
        eyre::bail!("sync returns error: {e:?}");
    }
    Ok(outcome.root_hash.unwrap_or([0; 32]))
}

/// Drive a round of CONCURRENT writes (one `(origin, method, params)` each),
/// then make every node apply every *other* node's delta in a fixed global
/// order. Returns each node's root hash after its LAST applied delta.
///
/// Each node ends having observed the identical SET of deltas (its own native
/// write + every other origin's delta), so a correct CRDT converges them to one
/// root regardless of which foreign delta each node happened to apply last —
/// that's exactly what `assert_converged` then checks. The per-node "last apply"
/// root is therefore the converged root, not order-sensitive, for a correct CRDT.
fn round(cluster: &mut Cluster, writes: &[(usize, &str, Value)]) -> Vec<[u8; 32]> {
    let deltas: Vec<(usize, Vec<u8>)> = writes
        .iter()
        .map(|(origin, method, params)| (*origin, cluster.call(*origin, method, params.clone())))
        .collect();

    let n = cluster.len();
    let mut roots = vec![None::<[u8; 32]>; n];
    for (node, root) in roots.iter_mut().enumerate() {
        for (origin, artifact) in &deltas {
            if *origin == node {
                continue;
            }
            let m = cluster.module;
            let h = apply_delta(m, &mut cluster.nodes[node], artifact)
                .unwrap_or_else(|e| panic!("apply on node {node} failed: {e}"));
            *root = Some(h);
        }
    }
    // Every node must have applied at least one foreign delta, otherwise its
    // root stays `None` and we'd silently compare a placeholder zero hash
    // against real hashes (masking a divergence). Enforce that invariant rather
    // than defaulting to `[0; 32]`: callers must give each node ≥1 foreign delta
    // (true for every concurrent round here — N writers, each node applies the
    // other N-1).
    roots
        .into_iter()
        .enumerate()
        .map(|(node, r)| {
            r.unwrap_or_else(|| {
                panic!(
                    "round(): node {node} applied no foreign delta — cannot determine its root; \
                     a concurrent round must give every node at least one other node's delta"
                )
            })
        })
        .collect()
}

/// Single-writer sync: node 0 performs the write, every other node applies its
/// delta. Returns all node root hashes (writer's WASM-computed hash first).
/// This is the basic "one node edits, peers receive the delta" path that every
/// CRDT must satisfy even before concurrent-merge reconciliation.
fn sync_one(cluster: &mut Cluster, method: &str, params: Value) -> Vec<[u8; 32]> {
    let (writer_root, artifact) = cluster.call_full(0, method, params);
    let mut roots = vec![writer_root];
    for node in 1..cluster.len() {
        let m = cluster.module;
        let h = apply_delta(m, &mut cluster.nodes[node], &artifact)
            .unwrap_or_else(|e| panic!("apply on node {node} failed: {e}"));
        roots.push(h);
    }
    roots
}

fn hx(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Assert every node converged to the same root hash.
fn assert_converged(label: &str, roots: &[[u8; 32]]) {
    let first = roots[0];
    for (i, r) in roots.iter().enumerate() {
        assert_eq!(
            &first,
            r,
            "{label}: node {i} diverged: {} vs node0 {}",
            hx(r),
            hx(&first)
        );
    }
}

// ===========================================================================
// Tests — one per data structure. Each does concurrent writes from 3 nodes
// (distinct keys so the writes commute) and asserts root-hash convergence.
// ===========================================================================

#[test]
fn kv_unordered_map_lww_converges() {
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "set", json!({"key": "a", "value": "1"})),
            (1, "set", json!({"key": "b", "value": "2"})),
            (2, "set", json!({"key": "c", "value": "3"})),
        ],
    );
    assert_converged("kv_set", &roots);

    // every node should see all three entries
    for n in 0..c.len() {
        let entries = c.query(n, "entries", json!({}));
        let ok = entries.get("output").unwrap_or(&entries);
        assert_eq!(ok["a"], json!("1"), "node {n} missing a");
        assert_eq!(ok["b"], json!("2"), "node {n} missing b");
        assert_eq!(ok["c"], json!("3"), "node {n} missing c");
    }
}

// The four tests below drive CONCURRENT writes to the SAME logical CRDT entity
// (one counter / one set key) — the hardest convergence case. These were once
// `#[ignore]`d because plain `StorageDelta::Actions` apply LWW-overwrote the
// CRDT container instead of merging it (concurrent writes carry distinct
// `updated_at`, so the merge only fired on the equal-timestamp branch). That
// gap has since been closed — `apply_action` now routes concurrent same-entity
// CRDT writes through the typed merge on the newer-timestamp branch too — so a
// GCounter correctly sums to 3. Kept as live regression coverage.
#[test]
fn gcounter_converges() {
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "increment_g_counter", json!({"key": "hits"})),
            (1, "increment_g_counter", json!({"key": "hits"})),
            (2, "increment_g_counter", json!({"key": "hits"})),
        ],
    );
    assert_converged("gcounter", &roots);
    let v = c.query(0, "get_g_counter", json!({"key": "hits"}));
    let n = v
        .get("output")
        .and_then(Value::as_u64)
        .or_else(|| v.as_u64());
    assert_eq!(n, Some(3), "GCounter should sum concurrent increments");
}

#[test]
fn pncounter_converges() {
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "increment_pn_counter", json!({"key": "balance"})),
            (1, "increment_pn_counter", json!({"key": "balance"})),
            (2, "decrement_pn_counter", json!({"key": "balance"})),
        ],
    );
    assert_converged("pncounter", &roots);
}

#[test]
fn nested_counter_in_map_converges() {
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "increment_counter", json!({"key": "k"})),
            (1, "increment_counter", json!({"key": "k"})),
            (2, "increment_counter", json!({"key": "k"})),
        ],
    );
    assert_converged("nested_counter", &roots);
}

#[test]
fn lww_register_in_map_converges() {
    // Concurrent writes to DISTINCT register keys (commute deterministically).
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "set_register", json!({"key": "r0", "value": "v0"})),
            (1, "set_register", json!({"key": "r1", "value": "v1"})),
            (2, "set_register", json!({"key": "r2", "value": "v2"})),
        ],
    );
    assert_converged("lww_register", &roots);
}

#[test]
fn vector_converges() {
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "push_metric", json!({"value": 10})),
            (1, "push_metric", json!({"value": 20})),
            (2, "push_metric", json!({"value": 30})),
        ],
    );
    assert_converged("vector_push", &roots);
}

#[test]
fn unordered_set_in_map_converges() {
    // crdt_tags: UnorderedMap<String, UnorderedSet<String>> — concurrent tag
    // additions to the SAME key from different nodes (set union).
    let mut c = Cluster::new(3);
    let roots = round(
        &mut c,
        &[
            (0, "add_tag", json!({"key": "post", "tag": "rust"})),
            (1, "add_tag", json!({"key": "post", "tag": "crdt"})),
            (2, "add_tag", json!({"key": "post", "tag": "wasm"})),
        ],
    );
    assert_converged("unordered_set_union", &roots);
    let count = c.query(0, "get_tag_count", json!({"key": "post"}));
    let n = count
        .get("output")
        .and_then(Value::as_u64)
        .or_else(|| count.as_u64());
    assert_eq!(n, Some(3), "set union should hold all three tags");
}

#[test]
fn rga_concurrent_appends_converge() {
    let mut c = Cluster::new(3);
    // Shared "Hello" prefix from the leader first, synced to all.
    let a = c.call(
        0,
        "rga_insert_text",
        json!({"position": 0, "text": "Hello"}),
    );
    c.apply(1, &a);
    c.apply(2, &a);

    let roots = round(
        &mut c,
        &[
            (0, "rga_append_text", json!({"text": " A"})),
            (1, "rga_append_text", json!({"text": " B"})),
            (2, "rga_append_text", json!({"text": " C"})),
        ],
    );
    assert_converged("rga_appends", &roots);

    // All nodes must agree on the linearized text too.
    let t0 = c.query(0, "rga_get_text", json!({}));
    for n in 1..c.len() {
        assert_eq!(
            t0,
            c.query(n, "rga_get_text", json!({})),
            "rga text node {n}"
        );
    }
}

#[test]
fn rga_delete_after_appends_converges() {
    let mut c = Cluster::new(3);
    let a = c.call(
        0,
        "rga_insert_text",
        json!({"position": 0, "text": "Hello"}),
    );
    c.apply(1, &a);
    c.apply(2, &a);

    let b = c.call(1, "rga_append_text", json!({"text": " brave new World"}));
    let cc = c.call(2, "rga_append_text", json!({"text": " of frozen bugs"}));
    c.apply(0, &b);
    c.apply(0, &cc);
    c.apply(2, &b);
    c.apply(1, &cc);

    // node 0 (the writer) deletes "Hello", broadcasts. Capture the WRITER's
    // own post-delete root hash (the WASM-computed one) via `call_full` — this
    // is the whole point of the scenario: the writer must not be the outlier.
    let (r0, d) = c.call_full(0, "rga_delete_text", json!({"start": 0, "end": 5}));
    let r1 = {
        let m = c.module;
        apply_delta(m, &mut c.nodes[1], &d).unwrap()
    };
    let r2 = {
        let m = c.module;
        apply_delta(m, &mut c.nodes[2], &d).unwrap()
    };
    // Writer (node 0) and both receivers must converge on the same root.
    assert_eq!(
        r0,
        r1,
        "rga delete: writer (node 0) diverged from receiver node 1: {} vs {}",
        hx(&r0),
        hx(&r1)
    );
    assert_eq!(
        r1,
        r2,
        "rga delete receivers diverged: {} vs {}",
        hx(&r1),
        hx(&r2)
    );

    let t1 = c.query(1, "rga_get_text", json!({}));
    let t2 = c.query(2, "rga_get_text", json!({}));
    let t0 = c.query(0, "rga_get_text", json!({}));
    assert_eq!(t0, t1, "text node0 vs node1");
    assert_eq!(t1, t2, "text node1 vs node2");
}

#[test]
fn frozen_storage_converges() {
    let mut c = Cluster::new(3);
    // Leader adds a frozen value, syncs to all; then concurrent additions.
    let a = c.call(0, "add_frozen", json!({"value": "Immutable"}));
    c.apply(1, &a);
    c.apply(2, &a);

    let roots = round(
        &mut c,
        &[
            (0, "add_frozen", json!({"value": "alpha"})),
            (1, "add_frozen", json!({"value": "beta"})),
            (2, "add_frozen", json!({"value": "gamma"})),
        ],
    );
    assert_converged("frozen", &roots);
}

/// Faithful in-process replay of the `frozen-rga-convergence.yml` e2e workflow
/// that intermittently fails CI with `DIVERGENCE DETECTED: Same DAG heads but
/// different root hash` on `__frozen_storage_frozen_items`. The workflow's
/// thesis: a frozen entry added early, then a later RGA edit storm + a title
/// change (LWW), makes one node's view of the *earlier frozen entry* diverge.
///
/// This drives the EXACT op order through real WASM (`Module::run`) over the
/// DELTA-apply path (`__calimero_sync_next`). If roots diverge here, the bug is
/// reproducible without the HashComparison whole-tree walk — i.e. in plain
/// delta apply. If they converge, the divergence is specific to the HC repair
/// path (whole-tree compare emitting an Update for the frozen leaf), narrowing
/// the search to the node sync layer.
#[test]
fn frozen_rga_e2e_sequence_converges() {
    let mut c = Cluster::new(3);

    // 1. node-1 adds the frozen entry; full-mesh broadcast.
    let f = c.call(0, "add_frozen", json!({"value": "Immutable data"}));
    c.apply(1, &f);
    c.apply(2, &f);

    // 2. node-1 seeds the RGA doc; broadcast.
    let ins = c.call(
        0,
        "rga_insert_text",
        json!({"position": 0, "text": "Hello"}),
    );
    c.apply(1, &ins);
    c.apply(2, &ins);

    // 3. CONCURRENT appends from node-2 and node-3 (each against the post-insert
    //    state, before seeing the other), then cross-apply both ways.
    let app1 = c.call(1, "rga_append_text", json!({"text": " brave new World"}));
    let app2 = c.call(2, "rga_append_text", json!({"text": " of frozen bugs"}));
    c.apply(0, &app1);
    c.apply(2, &app1);
    c.apply(0, &app2);
    c.apply(1, &app2);

    // 4. node-1 deletes "Hello" against its merged view; broadcast.
    let del = c.call(0, "rga_delete_text", json!({"start": 0, "end": 5}));
    c.apply(1, &del);
    c.apply(2, &del);

    // 5. The trigger: node-1 sets the document title (LWW). Capture the WRITER's
    //    own post-write root, then apply to the receivers and capture theirs.
    let (r0, title) = c.call_full(
        0,
        "rga_set_title",
        json!({"new_title": "E2E Test Document"}),
    );
    let r1 = {
        let m = c.module;
        apply_delta(m, &mut c.nodes[1], &title).unwrap()
    };
    let r2 = {
        let m = c.module;
        apply_delta(m, &mut c.nodes[2], &title).unwrap()
    };

    // The convergence check that fails in CI: all three roots must match.
    assert_eq!(
        r0,
        r1,
        "frozen+rga: writer node-1 diverged from node-2 after title change: {} vs {}",
        hx(&r0),
        hx(&r1)
    );
    assert_eq!(
        r1,
        r2,
        "frozen+rga: receivers diverged after title change: {} vs {}",
        hx(&r1),
        hx(&r2)
    );

    // Secondary: the frozen entry stays readable and identical everywhere
    // (the e2e's post-convergence assertions), and the RGA text converged.
    let t0 = c.query(0, "rga_get_text", json!({}));
    let t1 = c.query(1, "rga_get_text", json!({}));
    let t2 = c.query(2, "rga_get_text", json!({}));
    assert_eq!(t0, t1, "rga text node0 vs node1");
    assert_eq!(t1, t2, "rga text node1 vs node2");
}

// ---------------------------------------------------------------------------
// SortedMap ordered-index path, end to end through real WASM (core#2559).
//
// Unlike the storage crate's unit tests (which drive the adaptor natively or via
// a mock), this runs the compiled app through `Module::run`, so the ordered
// reads travel the FULL host-function chain: app WASM → `MainStorage` →
// `storage_index_set` / `storage_index_scan` host imports → the runtime's
// `InMemoryStorage` ordered index. A correct, key-ordered result here proves the
// Stage C host ABI works in a real WASM execution, not just in unit tests.
// ---------------------------------------------------------------------------

#[test]
fn sorted_map_range_through_real_wasm() {
    let mut c = Cluster::new(1);

    // Insert deliberately out of order — each `sorted_set` maintains the index
    // host-side via `storage_index_set`.
    for (k, v) in [
        ("delta", "D"),
        ("alpha", "A"),
        ("charlie", "C"),
        ("bravo", "B"),
        ("echo", "E"),
    ] {
        let _artifact = c.call(0, "sorted_set", json!({ "key": k, "value": v }));
    }

    // `sorted_range` reads back through `storage_index_scan` (the index-backed
    // path). The result must be the key-ordered window [bravo, echo).
    let range = c.query(
        0,
        "sorted_range",
        json!({ "start": "bravo", "end": "echo" }),
    );
    let range = range.get("output").cloned().unwrap_or(range);
    assert_eq!(
        range,
        json!({ "bravo": "B", "charlie": "C", "delta": "D" }),
        "sorted_range [bravo, echo) via the host ordered index"
    );

    // Full key listing comes back ascending too.
    let keys = c.query(0, "sorted_keys", json!({}));
    let keys = keys.get("output").cloned().unwrap_or(keys);
    assert_eq!(
        keys,
        json!(["alpha", "bravo", "charlie", "delta", "echo"]),
        "sorted_keys ascending"
    );

    // `last` is the reverse-seek path (`storage_index_last` host import).
    let last = c.query(0, "sorted_last_key", json!({}));
    let last = last.get("output").cloned().unwrap_or(last);
    assert_eq!(last, json!("echo"), "sorted_last_key via reverse seek");
}

/// A receiving node applies SortedMap entries via the generic sync path
/// (host-side, which does NOT touch the ordered index), so its index is stale.
/// The next `sorted_range` query must notice the `full_hash` marker mismatch,
/// rebuild the index from the synced entries, and return correct key-ordered
/// results — the self-heal-after-sync path, proven through real WASM.
#[test]
fn sorted_map_rebuilds_index_after_sync() {
    let mut c = Cluster::new(2);

    // Node 0 writes (out of order) and broadcasts each delta; node 1 applies
    // them via `__calimero_sync_next` (entries land host-side, index untouched).
    for (k, v) in [("m", "M"), ("a", "A"), ("z", "Z"), ("f", "F")] {
        let artifact = c.call(0, "sorted_set", json!({ "key": k, "value": v }));
        c.apply(1, &artifact);
    }

    // Node 1's ordered read must rebuild its stale index, then serve the window.
    let range = c.query(1, "sorted_range", json!({ "start": "a", "end": "z" }));
    let range = range.get("output").cloned().unwrap_or(range);
    assert_eq!(
        range,
        json!({ "a": "A", "f": "F", "m": "M" }),
        "node 1 rebuilds its index after sync and serves the range"
    );
}

// SortedSet ordered-index path, end to end through real WASM — the `SortedSet`
// counterpart of `sorted_map_range_through_real_wasm`. `sorted_tag_add`
// maintains the index host-side; `sorted_tags_range`/`sorted_tags_all`/
// `sorted_tags_last` read it back through the same host imports.
#[test]
fn sorted_set_range_through_real_wasm() {
    let mut c = Cluster::new(1);

    // Add deliberately out of order — the index is maintained host-side.
    for tag in ["delta", "alpha", "charlie", "bravo", "echo"] {
        let _artifact = c.call(0, "sorted_tag_add", json!({ "tag": tag }));
    }

    // Element-ordered window [bravo, echo) via `storage_index_scan`.
    let range = c.query(
        0,
        "sorted_tags_range",
        json!({ "start": "bravo", "end": "echo" }),
    );
    let range = range.get("output").cloned().unwrap_or(range);
    assert_eq!(
        range,
        json!(["bravo", "charlie", "delta"]),
        "sorted_tags_range [bravo, echo) via the host ordered index"
    );

    // Full listing ascending.
    let all = c.query(0, "sorted_tags_all", json!({}));
    let all = all.get("output").cloned().unwrap_or(all);
    assert_eq!(
        all,
        json!(["alpha", "bravo", "charlie", "delta", "echo"]),
        "sorted_tags_all ascending"
    );

    // Reverse-seek last (`storage_index_last`).
    let last = c.query(0, "sorted_tags_last", json!({}));
    let last = last.get("output").cloned().unwrap_or(last);
    assert_eq!(last, json!("echo"), "sorted_tags_last via reverse seek");
}

/// `SortedSet` self-heal-after-sync, mirroring `sorted_map_rebuilds_index_after_sync`:
/// a receiver applies elements via the generic sync path (index untouched), and
/// its next ordered read must rebuild the stale index and serve correct results.
#[test]
fn sorted_set_rebuilds_index_after_sync() {
    let mut c = Cluster::new(2);

    for tag in ["m", "a", "z", "f"] {
        let artifact = c.call(0, "sorted_tag_add", json!({ "tag": tag }));
        c.apply(1, &artifact);
    }

    let range = c.query(1, "sorted_tags_range", json!({ "start": "a", "end": "z" }));
    let range = range.get("output").cloned().unwrap_or(range);
    assert_eq!(
        range,
        json!(["a", "f", "m"]),
        "node 1 rebuilds its set index after sync and serves the range"
    );

    // `[a, z)` excludes `z`; assert the full set too so a bug that dropped the
    // last synced element (the range's exclusive bound) would still be caught.
    let all = c.query(1, "sorted_tags_all", json!({}));
    let all = all.get("output").cloned().unwrap_or(all);
    assert_eq!(
        all,
        json!(["a", "f", "m", "z"]),
        "node 1 has the full synced set in element order, including the last element"
    );
}

// ---------------------------------------------------------------------------
// Single-writer sync coverage. These exercise the basic "one node edits, peers
// apply the delta" path for the counter / set structures whose *concurrent*
// same-entity tests are ignored above — so every data structure has at least
// one green convergence test.
// ---------------------------------------------------------------------------

#[test]
fn gcounter_single_writer_syncs() {
    let mut c = Cluster::new(3);
    let roots = sync_one(&mut c, "increment_g_counter", json!({"key": "hits"}));
    assert_converged("gcounter_single", &roots);
    for n in 0..c.len() {
        let v = c.query(n, "get_g_counter", json!({"key": "hits"}));
        let got = v
            .get("output")
            .and_then(Value::as_u64)
            .or_else(|| v.as_u64());
        assert_eq!(got, Some(1), "node {n} GCounter after single increment");
    }
}

#[test]
fn pncounter_single_writer_syncs() {
    let mut c = Cluster::new(3);
    let roots = sync_one(&mut c, "increment_pn_counter", json!({"key": "balance"}));
    assert_converged("pncounter_single", &roots);
}

#[test]
fn nested_counter_single_writer_syncs() {
    let mut c = Cluster::new(3);
    let roots = sync_one(&mut c, "increment_counter", json!({"key": "k"}));
    assert_converged("nested_counter_single", &roots);
}

// ---------------------------------------------------------------------------
// Argument validation (core#1646): the macro-generated input struct now uses
// serde `deny_unknown_fields`, so passing an argument a method does not declare
// must fail loudly instead of being silently dropped.
// ---------------------------------------------------------------------------

/// Assert a call carrying an undeclared field fails with an "unknown field"
/// deserialize error naming that field. Uses the same `run()` helper as every
/// other test here (it bails when `outcome.returns` is `Err`, surfacing the
/// app's deserialize panic), so no manual borrowing of `Cluster` internals.
fn assert_unknown_field_rejected(c: &mut Cluster, method: &str, params: Value, field: &str) {
    let module = c.module;
    let err = run(module, &mut c.nodes[0], method, params)
        .expect_err("call with an unknown argument must fail, not silently ignore it");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("unknown field") && msg.contains(field),
        "expected an unknown-field deserialize error mentioning `{field}`, got: {msg}"
    );
}

/// A multi-arg (`set`) and a single-arg (`add_frozen`) method each reject an
/// extra field, while the same calls without it still succeed — i.e. the fix is
/// not specific to multi-field input structs.
#[test]
fn unknown_argument_is_rejected() {
    let mut c = Cluster::new(1);
    let module = c.module;

    assert_unknown_field_rejected(
        &mut c,
        "set",
        json!({"key": "k", "value": "v", "bogus": "x"}),
        "bogus",
    );
    assert_unknown_field_rejected(
        &mut c,
        "add_frozen",
        json!({"value": "v", "bogus": "x"}),
        "bogus",
    );

    // The declared arguments alone still deserialize and execute fine.
    run(
        module,
        &mut c.nodes[0],
        "set",
        json!({"key": "k", "value": "v"}),
    )
    .expect("valid multi-arg call should run");
    run(module, &mut c.nodes[0], "add_frozen", json!({"value": "v"}))
        .expect("valid single-arg call should run");
}

/// A method with no declared arguments (the macro's `args.is_empty()` branch)
/// rejects a populated JSON object — i.e. extra arguments it cannot consume —
/// but still accepts an empty `{}` or `null` body, which carry no named
/// arguments, so callers aren't forced to send a particular empty form
/// (core#2600).
#[test]
fn extra_fields_to_zero_arg_method_are_rejected() {
    let mut c = Cluster::new(1);
    let module = c.module;

    // `entries` declares no arguments; a field it cannot consume is rejected.
    assert_unknown_field_rejected(&mut c, "entries", json!({"bogus": "x"}), "bogus");

    // Empty-object and null bodies carry no arguments and must still succeed.
    run(module, &mut c.nodes[0], "entries", json!({}))
        .expect("zero-arg method accepts an empty object body");
    run(module, &mut c.nodes[0], "entries", json!(null))
        .expect("zero-arg method accepts a null body");
}

#[test]
fn unordered_set_single_writer_syncs() {
    let mut c = Cluster::new(3);
    let roots = sync_one(&mut c, "add_tag", json!({"key": "post", "tag": "rust"}));
    assert_converged("unordered_set_single", &roots);
    for n in 0..c.len() {
        let has = c.query(n, "has_tag", json!({"key": "post", "tag": "rust"}));
        let got = has
            .get("output")
            .and_then(Value::as_bool)
            .or_else(|| has.as_bool());
        assert_eq!(got, Some(true), "node {n} should have the synced tag");
    }
}
