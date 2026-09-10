//! Is the ~491-character single-call ceiling `rga_wall.rs` found for
//! `ReplicatedGrowableArray::insert_str` an RGA defect, or a property of the
//! whole storage layer?
//!
//! `tools/storage-cost/storage-costs.json` reports similar rows-touched-per-
//! entry across collections at n=10,000 (`unordered_map_insert` 66,
//! `unordered_set_insert` 66, `rga_insert` 66, `vector_push` 64,
//! `lww_register_set` 46). If gas is roughly proportional to rows touched,
//! then a per-call gas ceiling near ~500 entries should show up for EVERY
//! collection, not just RGA. This probe answers that directly, the same way
//! `rga_wall.rs::single_call_paste_wall` did: binary search, against a real
//! compiled guest, to an EXECUTED `GasExhausted`, not a projected one.
//!
//! # Guest
//!
//! `apps/bulk-write-bench` — purpose-built for this probe (no in-tree app
//! exposes a bulk-insert-into-an-empty-collection-in-one-call method for
//! `UnorderedMap`, `Vector`, or `UnorderedSet`). Each `insert_n_*` method
//! inserts `n` entries into an EMPTY collection in one call, so what's
//! measured is the flat per-entry cost, matching how `single_call_paste_wall`
//! measured RGA (paste into an empty document, not append to a long one).
//!
//! # Running it
//!
//!   cargo test -p calimero-runtime --test bulk_write_wall -- --ignored --nocapture
//!
//! # Measured results (2026-09-01, against this tree)
//!
//! See `.superpowers/sdd/2026-08-31-core-benchmark-suite/fidelity-3-report.md`
//! for the full table and the direct answer to the question above; the
//! short version is that all three walls land within a small multiple of
//! RGA's 491, so the ~500-entry ceiling is NOT RGA-specific.

use std::path::PathBuf;
use std::process::Command;

use calimero_account::AccountId;
use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
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

/// Build `bulk-write-bench` once per test-binary run (cached on disk,
/// rebuilt only when stale), mirroring `rga_wall.rs::editor_wasm`.
fn bench_wasm() -> Vec<u8> {
    let app_dir = workspace_root().join("apps/bulk-write-bench");
    let wasm_path = app_dir.join("res/bulk_write_bench.wasm");

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
            "building bulk-write-bench wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
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

/// Same discipline as `rga_wall.rs`: `GasExhausted` is the only failure that
/// is a measurement. Anything else means the call site drifted from the
/// app's real signature, and must never be reported as a wall.
enum Verdict {
    Wall { limit: u64 },
    Drift(String),
}

fn classify(method: &str, error: &FunctionCallError) -> Verdict {
    match error {
        FunctionCallError::GasExhausted { limit } => Verdict::Wall { limit: *limit },
        FunctionCallError::ExecutionError(payload) => Verdict::Drift(format!(
            "{method} returned an application error: {}\n\
             Compare the call site below against bulk-write-bench's current \
             #[app::logic] signature.",
            String::from_utf8_lossy(payload)
        )),
        other => Verdict::Drift(format!("{method} failed: {other}")),
    }
}

fn drift(detail: &str) -> ! {
    panic!(
        "\n\
         ==================== CONTRACT DRIFT — NOT A STORAGE RESULT ====================\n\
         {detail}\n\
         \n\
         Nothing has been measured and no wall has been found; fix the call site in this \
         file, then rerun.\n\
         ==============================================================================\n"
    )
}

fn expect_ok(outcome: &Outcome, method: &str) {
    if let Err(error) = &outcome.returns {
        match classify(method, error) {
            Verdict::Drift(detail) => drift(&detail),
            Verdict::Wall { limit } => drift(&format!(
                "{method} exhausted its {limit}-point gas budget on the FIRST call, \
                 with n=0. That is not a wall — a wall needs entries behind it."
            )),
        }
    }
}

fn preflight(module: &calimero_runtime::Module, method: &str) {
    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    expect_ok(
        &call(
            module,
            &mut storage,
            method,
            &serde_json::json!({"n": 1_u32}),
        ),
        method,
    );
}

/// The single-call ceiling of one bulk-insert method: the largest `n` that
/// lands into an EMPTY collection in ONE call before that call itself
/// exhausts gas. Returns `(largest_landed, gas_at_largest_landed,
/// storage_reads_at_largest_landed, storage_writes_at_largest_landed,
/// first_exhausted)`.
fn find_wall(module: &calimero_runtime::Module, method: &str) -> (usize, u64, u64, u64, usize) {
    preflight(module, method);

    // Seed a bracket wide enough to survive a big shift in the app's cost
    // model without silently mis-measuring: `lo` known to land, `hi` known to
    // wall. Checked once, not searched for — if the model shifts far enough
    // that 8,192 no longer walls, that is itself worth failing loudly on
    // rather than silently widening past it.
    let mut lo = 1_usize;
    let mut hi = 8_192_usize;
    {
        let mut storage = InMemoryStorage::default();
        expect_ok(
            &call(module, &mut storage, "init", &serde_json::json!({})),
            "init",
        );
        let outcome = call(
            module,
            &mut storage,
            method,
            &serde_json::json!({"n": hi as u32}),
        );
        match &outcome.returns {
            Ok(_) => drift(&format!(
                "a single {method} call with n={hi} into an EMPTY collection succeeded. \
                 The seeded upper bound for this search is no longer past the wall — \
                 raise it in this file."
            )),
            Err(error) => match classify(method, error) {
                Verdict::Wall { .. } => {}
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    }

    let mut lo_gas = 0_u64;
    let mut lo_reads = 0_u64;
    let mut lo_writes = 0_u64;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let mut storage = InMemoryStorage::default();
        expect_ok(
            &call(module, &mut storage, "init", &serde_json::json!({})),
            "init",
        );
        let outcome = call(
            module,
            &mut storage,
            method,
            &serde_json::json!({"n": mid as u32}),
        );
        match &outcome.returns {
            Ok(_) => {
                lo = mid;
                lo_gas = outcome.gas_used.unwrap_or(0);
                lo_reads = outcome.storage_reads;
                lo_writes = outcome.storage_writes;
            }
            Err(error) => match classify(method, error) {
                Verdict::Wall { .. } => hi = mid,
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    }

    (lo, lo_gas, lo_reads, lo_writes, hi)
}

/// Executed (not extrapolated) single-call write walls for all three
/// collections `bulk-write-bench` exposes, answering directly whether RGA's
/// ~491-character wall is RGA-specific or platform-wide.
#[test]
#[ignore = "slow: builds the compiled bulk-write-bench app and binary-searches three \
            single-call gas walls. Fast to execute once built (well under a second \
            per collection)."]
fn single_call_write_walls() {
    let wasm = bench_wasm();
    let limits = VMLimits::default();
    println!("guest:   bulk-write-bench, built from this tree");
    println!("max_gas: {:?}", limits.max_gas);

    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    println!(
        "\n  collection      largest n lands   gas at largest n   reads   writes   \
         gas/row   first n exhausts"
    );
    let mut results = Vec::new();
    for (label, method) in [
        ("UnorderedMap", "insert_n_map"),
        ("Vector", "insert_n_vec"),
        ("UnorderedSet", "insert_n_set"),
    ] {
        let (lo, lo_gas, lo_reads, lo_writes, hi) = find_wall(&module, method);
        let rows = lo_reads + lo_writes;
        let gas_per_row = if rows > 0 {
            lo_gas as f64 / rows as f64
        } else {
            0.0
        };
        println!(
            "  {label:<14}  {lo:>15}   {lo_gas:>17}   {lo_reads:>5}   {lo_writes:>6}   \
             {gas_per_row:>7.1}   {hi:>17}"
        );
        results.push((label, lo, lo_gas, hi));
    }

    for (label, lo, _gas, hi) in &results {
        assert!(
            *lo >= 10,
            "{label} walled after only {lo} entries — investigate before quoting the \
             number, this is far below anything expected"
        );
        assert_eq!(
            *hi,
            lo + 1,
            "{label}: binary search did not converge to adjacent lo/hi"
        );
    }
}

/// The discriminator itself runs in CI, even though the sweep above does
/// not. Everything above rests on `classify` telling a measured wall apart
/// from a stale call signature.
#[test]
fn only_gas_exhaustion_counts_as_a_wall() {
    assert!(
        matches!(
            classify(
                "insert_n_map",
                &FunctionCallError::GasExhausted { limit: 1_000 }
            ),
            Verdict::Wall { limit: 1_000 }
        ),
        "gas exhaustion is the one failure that is a measurement"
    );

    let Verdict::Drift(detail) = classify(
        "insert_n_map",
        &FunctionCallError::ExecutionError(b"missing field `n`".to_vec()),
    ) else {
        panic!(
            "an application error was classified as a wall — the probe would report \
             a fictional number"
        );
    };
    assert!(
        detail.contains('n'),
        "the drift message must carry the app's own complaint, got: {detail}"
    );

    let Verdict::Drift(detail) = classify(
        "insert_n_map",
        &FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            name: "insert_n_map".to_owned(),
        }),
    ) else {
        panic!("a renamed method was classified as a wall");
    };
    assert!(
        detail.contains("insert_n_map"),
        "the drift message must name the method, got: {detail}"
    );
}
