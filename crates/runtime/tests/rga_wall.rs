//! Where a real `ReplicatedGrowableArray` document stops being writable, and
//! separately where it stops being readable at all.
//!
//! Unlike `cost_is_flat`, which uses a synthetic guest so that any growth it
//! measures is unambiguously a storage defect, this wants the number a user of
//! a real editor hits, so it drives `apps/collaborative-editor` from this
//! workspace. It is `#[ignore]`d only because reaching a wall this large means
//! executing thousands of real WASM calls.
//!
//! `insert_str` re-derives the position by linearising the WHOLE document on
//! every call, so typing is `O(n)` per call and `O(n^2)` in total; once one
//! `insert_text` exceeds `max_gas`, every later call does too and the document
//! is permanently unwritable. `get_text()` performs the same linearisation and
//! so has its own wall, measured separately: a document that still accepts
//! writes but can no longer be opened is just as dead.
//!
//! `GasExhausted` is the only outcome that produces a number; anything else
//! panics as contract drift, naming the method. A preflight against an empty
//! document reports drift in seconds rather than after a long sweep.
//!
//! Measured against this tree (2026-09-19) under a 1,000,000,000 gas ceiling:
//! typing and `get_text` both wall at ~17,100 characters, and mid-document
//! typing at ~17,900, so position buys nothing (all three extrapolated from a
//! 17,000-character sweep). One `insert_text` into an empty document takes 742
//! characters, executed by binary search; batching buys chunkier calls rather
//! than a different asymptote, since every later call still re-linearises.
//!
//!   cargo test -p calimero-runtime --test rga_wall -- --ignored --nocapture
//!
//! Raise the ceiling if the wall has moved out of the default range:
//!   RGA_WALL_CEILING=20000 cargo test ... -- --ignored --nocapture

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use calimero_account::AccountId;
use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
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

/// Newest mtime across the app's build inputs, so this probe never silently
/// measures a stale binary. Mirrors `tracing_logs.rs`'s fixture.
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
/// rebuilt only when stale) and return its wasm bytes.
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

/// What a failed call means. As in `chat_wall`: a measured wall and a broken
/// harness must never be reported the same way.
enum Verdict {
    /// The guest ran and exhausted its budget. This is a result.
    Wall { limit: u64 },
    /// The guest did not get far enough to cost anything meaningful. This is
    /// not a result, and must never be presented as one.
    Drift(String),
}

/// Classify a failed outcome. `GasExhausted` is the ONLY failure that counts
/// as a wall.
fn classify(method: &str, error: &FunctionCallError) -> Verdict {
    match error {
        FunctionCallError::GasExhausted { limit } => Verdict::Wall { limit: *limit },
        FunctionCallError::ExecutionError(payload) => Verdict::Drift(format!(
            "{method} returned an application error: {}\n\
             The usual cause is an argument this probe passes that the app no longer \
             takes. Compare the call site below against collaborative-editor's current \
             #[app::logic] signature.",
            String::from_utf8_lossy(payload)
        )),
        other => Verdict::Drift(format!("{method} failed: {other}")),
    }
}

fn drift(detail: &str) -> ! {
    panic!(
        "\n\
         ==================== CONTRACT DRIFT - NOT A STORAGE RESULT ====================\n\
         {detail}\n\
         \n\
         Nothing has been measured and no wall has been found; fix the call site in this \
         file, then rerun.\n\
         \n\
         The in-repo gate for the related property does not depend on this app:\n\
         \x20 cargo test -p storage-cost      (rga_insert_per_char, rga_get_nth)\n\
         \x20 ./scripts/check-storage-cost.sh\n\
         ==============================================================================\n"
    )
}

fn expect_ok(outcome: &Outcome, method: &str) {
    if let Err(error) = &outcome.returns {
        match classify(method, error) {
            Verdict::Drift(detail) => drift(&detail),
            Verdict::Wall { limit } => drift(&format!(
                "{method} exhausted its {limit}-point gas budget on the FIRST call, \
                 against an empty document. That is not a wall - a wall needs data \
                 behind it. Either max_gas has been lowered dramatically or the app now \
                 does unbounded work at n=0."
            )),
        }
    }
}

/// Prove the app still answers the calls this probe makes, on a THROWAWAY
/// store, before spending minutes on a sweep.
fn preflight(module: &calimero_runtime::Module) {
    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    expect_ok(
        &call(
            module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": "a"}),
        ),
        "insert_text",
    );
    let read = call(module, &mut storage, "get_text", &serde_json::json!({}));
    expect_ok(&read, "get_text");
    // `get_text` returns the document as a bare JSON string, not wrapped in an
    // "output" envelope: confirmed against the compiled app, not assumed.
    let body = read
        .returns
        .as_ref()
        .unwrap_or_else(|_| drift("get_text failed during preflight"))
        .as_ref()
        .unwrap_or_else(|| drift("get_text returned no value at all"));
    let text: String = serde_json::from_slice(body)
        .unwrap_or_else(|e| drift(&format!("get_text did not return a JSON string: {e}")));
    if text != "a" {
        drift(&format!(
            "after one insert_text(0, \"a\"), get_text() returned {text:?}, not \"a\".              The write is not landing where the read looks, so every number this probe              would produce is an artifact."
        ));
    }
}

/// Stop even if nothing walls, so a flat build cannot run forever.
const DEFAULT_CEILING: usize = 20_000;

fn ceiling() -> usize {
    std::env::var("RGA_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

/// The document ceiling: type one character at a time until a call exhausts
/// gas, probing `get_text` as the document grows.
///
/// The `i_reads`/`r_reads` columns are one call's end-to-end host reads, which
/// include root-state fetch/commit, `Counter::increment` and SDK marshaling.
/// They are not comparable with `rga_insert_per_char`'s committed `n + 47`
/// reads/entry, which averages direct in-process calls over a whole build.
#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            collaborative-editor app to find where insert_text/get_text actually \
            exhaust gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (rga_insert_per_char, rga_get_nth)."]
fn typing_and_reading_walls() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    println!("guest:   real collaborative-editor app, built from this tree");
    println!("max_gas: {:?}", limits.max_gas);

    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    preflight(&module);

    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );

    let ceiling = ceiling();
    // Read is measured every 100 characters: cheap relative to a keystroke at
    // these sizes, and frequent enough to bracket the read wall tightly.
    const READ_PROBE_STRIDE: usize = 100;

    let mut landed = 0_usize;
    let mut write_wall: Option<usize> = None;
    let mut last_read_ok: Option<usize> = None;
    let mut read_wall: Option<usize> = None;

    println!("\n  n        insert_gas       i_reads   ms  | get_text_gas    r_reads   ms");

    for i in 0..ceiling {
        let started = Instant::now();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": i, "text": "x"}),
        );
        let write_ms = started.elapsed().as_secs_f64() * 1000.0;

        if let Err(error) = &outcome.returns {
            match classify("insert_text", error) {
                Verdict::Wall { limit } => {
                    println!("\nWRITE WALL at {landed} (gas limit {limit})");
                    write_wall = Some(landed);
                    break;
                }
                Verdict::Drift(detail) => drift(&format!(
                    "{detail}\nThis appeared only after {landed} characters, so it is \
                     state-dependent rather than a stale call signature."
                )),
            }
        }
        landed += 1;

        if read_wall.is_none() && landed.is_multiple_of(READ_PROBE_STRIDE) {
            let started = Instant::now();
            let read = call(&module, &mut storage, "get_text", &serde_json::json!({}));
            let read_ms = started.elapsed().as_secs_f64() * 1000.0;

            match &read.returns {
                Ok(_) => {
                    last_read_ok = Some(landed);
                    println!(
                        "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1} | {:>12?}  {:>7}  {read_ms:>5.1}",
                        outcome.gas_used, outcome.storage_reads, read.gas_used, read.storage_reads,
                    );
                }
                Err(error) => match classify("get_text", error) {
                    Verdict::Wall { .. } => {
                        read_wall = Some(landed);
                        println!(
                            "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1} | READ WALL (gas exhausted)",
                            outcome.gas_used, outcome.storage_reads,
                        );
                    }
                    Verdict::Drift(detail) => drift(&detail),
                },
            }
        }
    }

    println!("\n--- result ---");
    println!("characters landed:  {landed}");
    match write_wall {
        Some(n) => println!("write wall (insert_text, one more char): {n}"),
        None => println!("write wall (insert_text, one more char): none below {ceiling}"),
    }
    match (last_read_ok, read_wall) {
        (Some(ok), Some(bad)) => {
            println!("read wall  (get_text): last OK at {ok}, first exhausted at {bad}")
        }
        (_, Some(bad)) => println!("read wall  (get_text): exhausted by {bad}"),
        (_, None) => println!("read wall  (get_text): none below {landed}"),
    }

    assert!(
        landed > 0,
        "no character was inserted even though preflight succeeded - the failure \
         classification in this file is broken"
    );

    if let Some(n) = write_wall.or(read_wall) {
        assert!(
            n >= 50,
            "walled after only {n} characters. Preflight passed, so the calls are \
             well-formed, but a ceiling this low is a change in the app's work per \
             call, not the cost curve this probe exists to measure. Investigate before \
             quoting the number."
        );
    }
}

/// The mid-document write ceiling: type one character at a time into the
/// middle of the document (`position: landed / 2`) until a call exhausts gas.
///
/// Kept identical to `fugue_wall.rs`'s `mid_document_typing_wall` so the two
/// ceilings are comparable. It is the control for that one: `get_ordered_chars`
/// linearises the whole document whatever the position, so RGA has no append
/// fast path to lose and should land at the append ceiling.
#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            collaborative-editor app to find where a MID-DOCUMENT insert_text \
            exhausts gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (rga_insert_middle)."]
fn mid_document_typing_wall() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    println!("guest:   real collaborative-editor app, built from this tree");
    println!("max_gas: {:?}", limits.max_gas);
    println!("position: landed / 2 (mid-document), NOT an append");

    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    preflight(&module);

    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );

    let ceiling = ceiling();
    const PROBE_STRIDE: usize = 100;

    let mut landed = 0_usize;
    let mut write_wall: Option<usize> = None;

    println!("\n  n        insert_gas       i_reads   ms");

    for _ in 0..ceiling {
        let started = Instant::now();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": landed / 2, "text": "x"}),
        );
        let write_ms = started.elapsed().as_secs_f64() * 1000.0;

        if let Err(error) = &outcome.returns {
            match classify("insert_text", error) {
                Verdict::Wall { limit } => {
                    println!("\nMID-DOCUMENT WRITE WALL at {landed} (gas limit {limit})");
                    write_wall = Some(landed);
                    break;
                }
                Verdict::Drift(detail) => drift(&format!(
                    "{detail}\nThis appeared only after {landed} characters, so it is \
                     state-dependent rather than a stale call signature."
                )),
            }
        }
        landed += 1;

        if landed.is_multiple_of(PROBE_STRIDE) {
            println!(
                "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1}",
                outcome.gas_used, outcome.storage_reads,
            );
        }
    }

    println!("\n--- result ---");
    println!("characters landed:  {landed}");
    match write_wall {
        Some(n) => println!("mid-document write wall (insert_text at landed/2): {n}"),
        None => println!("mid-document write wall: none below {ceiling}"),
    }

    assert!(
        landed > 0,
        "no character was inserted even though preflight succeeded - the failure \
         classification in this file is broken"
    );
    if let Some(n) = write_wall {
        assert!(
            n >= 50,
            "walled after only {n} mid-document characters. Preflight passed, so the \
             calls are well-formed, but a ceiling this low is a change in the app's work \
             per call, not the cost curve this probe exists to measure."
        );
    }
}

/// How much NEW text ONE call can add, a different question from the write
/// wall above: here the linearise cost is paid against an empty document, so
/// the flat per-character insert cost dominates instead.
#[test]
#[ignore = "slow: builds the compiled collaborative-editor app. Fast to execute \
            once built (well under a second), unlike the sweep above."]
fn single_call_paste_wall() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    preflight(&module);

    // Binary search: `lo` is known to land, `hi` is known to wall.
    let mut lo = 1_usize;
    // Above the observed wall but below the 16 KiB `app::log!` line-length
    // limit, which a larger seed would trip first and misreport as the gas wall.
    let mut hi = 8_192;
    let mut hi_confirmed = false;
    while !hi_confirmed {
        let mut storage = InMemoryStorage::default();
        expect_ok(
            &call(&module, &mut storage, "init", &serde_json::json!({})),
            "init",
        );
        let text: String = std::iter::repeat_n('a', hi).collect();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": text}),
        );
        match &outcome.returns {
            Ok(_) => drift(&format!(
                "a single insert_text call of {hi} characters into an EMPTY document \
                 succeeded. The seeded upper bound for this search is no longer past \
                 the wall; raise it in this file."
            )),
            Err(error) => match classify("insert_text", error) {
                Verdict::Wall { .. } => hi_confirmed = true,
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    }

    let mut lo_gas: Option<u64> = None;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let mut storage = InMemoryStorage::default();
        expect_ok(
            &call(&module, &mut storage, "init", &serde_json::json!({})),
            "init",
        );
        let text: String = std::iter::repeat_n('a', mid).collect();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": text}),
        );
        match &outcome.returns {
            Ok(_) => {
                lo = mid;
                lo_gas = outcome.gas_used;
            }
            Err(error) => match classify("insert_text", error) {
                Verdict::Wall { .. } => hi = mid,
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    }

    println!("single-call paste wall: {lo} characters land ({lo_gas:?} gas), {hi} exhausts gas");
    assert!(
        lo >= 10,
        "the single-call paste wall landed at only {lo} characters - investigate \
         before quoting the number, this is far below anything seen so far"
    );
}

/// The discriminator itself runs in CI, even though the probes above do not:
/// everything here rests on `classify` telling a measured wall apart from a
/// stale call signature, and it has no other coverage.
#[test]
fn only_gas_exhaustion_counts_as_a_wall() {
    assert!(
        matches!(
            classify(
                "insert_text",
                &FunctionCallError::GasExhausted { limit: 1_000 }
            ),
            Verdict::Wall { limit: 1_000 }
        ),
        "gas exhaustion is the one failure that is a measurement"
    );

    let Verdict::Drift(detail) = classify(
        "insert_text",
        &FunctionCallError::ExecutionError(b"missing field `text`".to_vec()),
    ) else {
        panic!(
            "an application error was classified as a wall - the probe would report \
             a fictional number"
        );
    };
    assert!(
        detail.contains("text"),
        "the drift message must carry the app's own complaint, got: {detail}"
    );

    let Verdict::Drift(detail) = classify(
        "get_text",
        &FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            name: "get_text".to_owned(),
        }),
    ) else {
        panic!("a renamed method was classified as a wall");
    };
    assert!(
        detail.contains("get_text"),
        "the drift message must name the method, got: {detail}"
    );
}
