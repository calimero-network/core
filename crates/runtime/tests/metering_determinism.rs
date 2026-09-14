//! Gas for a given logical operation must not depend on node-local derived
//! state, or two replicas that took the same write can disagree about
//! whether it happened: one has warm/populated local caches and lands the
//! call inside `max_gas`, the other is cold and traps with `GasExhausted` —
//! which is silent divergence, not slowness.
//!
//! # Why this passes today, and why that is not vacuous
//!
//! There is no node-local derived state on the write path today —
//! `ReplicatedGrowableArray::insert_str` re-derives everything it needs by
//! linearising the stored chars on every call (see `rga_wall.rs`'s module
//! doc), so nothing here varies between a "cold" and a "warm" run and this
//! test is expected to PASS on day one. That is intentional, not a weak
//! test: its job is not to discriminate today, it is to stand as a forward
//! tripwire. The moment a future change adds a node-local cache, index, or
//! other derived state that the write path reads (an insertion-point cache,
//! a length cache, anything seeded from prior reads rather than recomputed
//! from the stored data), this test starts failing — which is exactly the
//! point at which such a change would also introduce cross-replica gas
//! divergence. Do not delete this test for being "trivial"; read this
//! comment first.
//!
//! # Why the comparison below is a tolerance, not `assert_eq!`
//!
//! `insert_text`'s `insert_str` call draws a fresh HLC timestamp
//! (`env::hlc_timestamp()`) for every insert. The HLC's *id* component is
//! deterministic (seeded from the device id, see
//! `calimero_storage::env`'s `ensure_hlc_initialized`), but its *physical
//! time* component reads the real wall clock (`env::time_now()`), quantised
//! to roughly 15-microsecond buckets. Two `insert_text` calls issued even
//! microseconds apart land in different buckets, so the newly-inserted
//! character's `CharId` differs at the byte level between ANY two runs —
//! including two runs that are otherwise byte-for-byte identical. That new
//! key's exact bytes influence how the storage layer's own indexing lays it
//! out, which can move a handful of bytes read (not entries — the entry
//! *count* was observed to match exactly across many repeated trials, only
//! the byte length of a couple of those reads varies) and therefore gas, by
//! a small amount unrelated to node-local caching.
//!
//! This was verified empirically before writing the tolerance: comparing
//! two structurally-identical `cold`-vs-`cold` measurements (same seed
//! document, cloned from one build, zero warm-up either side) still
//! disagreed on gas in roughly half of ~20 trials, always by well under 1%
//! (worst observed: 10,625 gas out of ~5.88M, ≈0.18%). A caching regression
//! of the kind this test exists to catch is a qualitatively different
//! effect — turning an `O(1)`-per-call cost into something that reads
//! stale/derived state and skips work, or the reverse — and would not land
//! inside a sub-1% band by coincidence (`rga_wall.rs` shows the underlying
//! per-call cost scales at ~80,000+ gas per character of document length;
//! a real node-local-state dependency would show up as a shift of that
//! order, not wall-clock noise). `TOLERANCE_FRACTION` below is set to 5×
//! the worst noise observed, leaving ample room before a real regression
//! could hide in it while still ruling out exact-equality flakiness that
//! has nothing to do with the property under test.
//!
//! # Harness
//!
//! Reuses `rga_wall.rs`'s idiom of driving the real `apps/collaborative-editor`
//! guest through `calimero_runtime::Engine`/`Module::run` rather than a
//! synthetic guest, because the property under test — gas independence from
//! node-local state — has to be checked against the actual `insert_text`
//! call path (RGA linearisation, `#[app::state]` fetch/commit, SDK
//! marshaling and all), not a stripped-down stand-in that might not exercise
//! the same reads. Unlike `rga_wall.rs::typing_and_reading_walls`, this does
//! a handful of calls rather than a sweep to a gas wall, so it runs as a
//! normal (non-`#[ignore]`d) test.
//!
//! Both `cold` and `warm` measurements clone their starting storage from a
//! single shared seeded build (see [`seeded_module_and_storage`]) rather
//! than each building their own "hello world" seed independently — building
//! independently would reintroduce the same wall-clock-timestamp confound
//! described above into the SEED content as well, on top of the measured
//! insert.

use std::path::PathBuf;
use std::process::Command;

use calimero_account::AccountId;
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

fn workspace_root() -> PathBuf {
    // crates/runtime/ -> ../../
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Newest mtime across the app's build inputs. Mirrors `rga_wall.rs`'s
/// fixture: rebuild whenever any `*.rs` under `src/`, `Cargo.toml`, or
/// `build.rs` is newer than the last build, so this test never silently
/// measures a stale binary.
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

/// Build `collaborative-editor` once per test-binary run (cached on disk,
/// rebuilt only when stale), mirroring `rga_wall.rs::editor_wasm`.
fn editor_wasm() -> Vec<u8> {
    let app_dir = workspace_root().join("apps/collaborative-editor");
    let wasm_path = app_dir.join("res/collaborative_editor.wasm");

    let wasm_mtime = std::fs::metadata(&wasm_path)
        .and_then(|m| m.modified())
        .ok();
    let needs_build = match (wasm_mtime, newest_mtime(&app_dir)) {
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
            "building collaborative-editor wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    std::fs::read(&wasm_path).unwrap_or_else(|e| panic!("{}: {e}", wasm_path.display()))
}

fn call(
    module: &calimero_runtime::Module,
    storage: &mut InMemoryStorage,
    method: &str,
    args: &serde_json::Value,
) -> Outcome {
    module
        .run(
            [0_u8; 32].into(),
            AccountId::from([0_u8; 32]),
            [0_u8; 32].into(),
            method,
            &serde_json::to_vec(args).expect("encode args"),
            storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

fn expect_ok(outcome: &Outcome, method: &str) {
    assert!(
        outcome.returns.is_ok(),
        "{method} failed: {:?}",
        outcome.returns
    );
}

/// Compile the guest once and build ONE seeded document (`init`, then a
/// single `insert_text` of "hello world" at position 0), returning the
/// module and the resulting storage so both the `cold` and `warm` branches
/// of [`measure_insert_gas`] start from byte-identical prior state — cloned
/// from this one build, not rebuilt independently. See the module doc's
/// "Why the comparison below is a tolerance" section for why rebuilding the
/// seed independently per branch would reintroduce wall-clock-timestamp
/// noise into content that is supposed to be identical either way.
fn seeded_module_and_storage() -> (calimero_runtime::Module, InMemoryStorage) {
    let wasm = editor_wasm();
    let module = Engine::with_limits(VMLimits::default())
        .compile(&wasm)
        .expect("compile metered module");

    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    expect_ok(
        &call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": "hello world"}),
        ),
        "insert_text (seed)",
    );
    (module, storage)
}

/// Measure the gas cost of ONE `insert_text` call at a fixed position into a
/// document that has been built to the same content either way (`storage`
/// is cloned from a single shared build — see [`seeded_module_and_storage`]),
/// differing only in whether the replica's local state was exercised
/// beforehand.
///
/// `cold`: the measured insert happens immediately against the cloned seed
/// — no reads precede it, so any node-local cache would still be empty.
///
/// `!cold` ("warm"): before the measured insert, the document is read
/// several times (`get_text`, `get_stats`, `get_length`) — the kind of
/// activity that would populate a node-local cache or derived index, if one
/// existed. The measured insert is otherwise identical: same prior document,
/// same position, same inserted text.
fn measure_insert_gas(
    module: &calimero_runtime::Module,
    storage: &InMemoryStorage,
    cold: bool,
) -> u64 {
    let mut storage = storage.clone();

    if !cold {
        // Exercise reads over the seeded document — the activity that would
        // populate a node-local cache or derived index, if the write path
        // had one to consult.
        for _ in 0..5 {
            expect_ok(
                &call(module, &mut storage, "get_text", &serde_json::json!({})),
                "get_text (warm-up)",
            );
            expect_ok(
                &call(module, &mut storage, "get_stats", &serde_json::json!({})),
                "get_stats (warm-up)",
            );
            expect_ok(
                &call(module, &mut storage, "get_length", &serde_json::json!({})),
                "get_length (warm-up)",
            );
        }
    }

    let outcome = call(
        module,
        &mut storage,
        "insert_text",
        &serde_json::json!({"position": 5_usize, "text": "!"}),
    );
    expect_ok(&outcome, "insert_text (measured)");
    outcome
        .gas_used
        .expect("a successful outcome always reports gas_used")
}

/// 5x the worst wall-clock-driven gas noise observed across ~20 repeated
/// `cold`-vs-`cold` trials (0.18%) run while developing this test — see the
/// module doc's "Why the comparison below is a tolerance" section. A real
/// node-local-caching regression is expected to land far outside this band.
const TOLERANCE_FRACTION: f64 = 0.01;

/// The same logical operation must consume the same gas regardless of node-local
/// derived state. If it does not, one replica can complete a write while another traps
/// on GasExhausted — which is divergence, not slowness.
///
/// This is a forward tripwire, not a discriminating test today: see the
/// module doc comment for why `cold ≈ warm` here is the correct, expected
/// result against the current write path, and why that does not make the
/// assertion vacuous. The comparison allows a small tolerance for wall-clock
/// timestamp noise that is unrelated to node-local state — see the module
/// doc's "Why the comparison below is a tolerance" section for the measured
/// justification.
#[test]
fn insert_gas_is_independent_of_local_derived_state() {
    let (module, storage) = seeded_module_and_storage();
    let cold = measure_insert_gas(&module, &storage, /* node-local caches empty */ true);
    let warm = measure_insert_gas(
        &module, &storage, /* node-local caches populated */ false,
    );
    println!("cold gas: {cold}");
    println!("warm gas: {warm}");

    let diff = cold.abs_diff(warm);
    #[expect(
        clippy::cast_precision_loss,
        reason = "gas values are well under f64's exact-integer range; a\
                   tolerance check does not need bit-exact precision"
    )]
    let allowed = (cold.max(warm) as f64 * TOLERANCE_FRACTION) as u64;
    assert!(
        diff <= allowed,
        "insert consumed {cold} gas cold and {warm} gas warm (diff {diff}, allowed \
         {allowed}); a gas difference this large is far beyond the wall-clock-timestamp \
         noise floor this test tolerates, and means node-local state now affects insert \
         gas — two replicas can disagree about whether a write happened"
    );
}
