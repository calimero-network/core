//! Where a real `FugueText` document stops being writable, and separately
//! where it stops being readable at all.
//!
//! The `FugueText` counterpart of `rga_wall.rs`, and deliberately its mirror:
//! same harness, same failure classification, same sweep, so the two numbers
//! it produces are comparable with the two that one produces. Read that file's
//! module docs for the reasoning behind the shape; what follows is only what
//! differs.
//!
//! # A different app, for the same reason
//!
//! `rga_wall.rs` drives `apps/collaborative-editor`, whose `insert_text` maps
//! one keystroke onto `ReplicatedGrowableArray::insert_str` with a
//! one-character string. This drives `apps/fugue-editor`, which exists solely
//! as that app's `FugueText` twin: same state fields, same per-call work in
//! `insert_text` (log, insert, `Counter::increment`, emit), only the
//! collection differs. A wall measured against a synthetic guest would not be
//! the number a user hits, and a wall measured against a DIFFERENT harness
//! would not be comparable to RGA's.
//!
//! # Two write sweeps, not one
//!
//! `typing_and_reading_walls` types at `position: i`, which is always the END
//! of the document — an append, the case run-length blocks coalesce — and
//! `single_call_paste_wall` pastes into an EMPTY document. Neither touches
//! mid-document insertion, which is the operation `FugueText`'s block layout
//! most affects, so `mid_document_typing_wall` sweeps that separately.
//!
//! # Three reads, not one
//!
//! `ReplicatedGrowableArray` has exactly one read — `get_text()`, which
//! materialises the whole document — so `rga_wall.rs` has exactly one read
//! wall to find. `FugueText` also answers `char_at` and `text_range`, so this
//! probe sweeps all three: a positional read that keeps working after the
//! whole-document read has died is the capability difference, expressed as a
//! wall rather than as a row count.
//!
//! # Measured results
//!
//! Recorded in `.superpowers/sdd/2026-09-02-rga-fugue-rework/stack6-report.md`
//! alongside RGA's, rather than transcribed here, so the pair is read
//! together. Run it yourself with:
//!
//!   cargo test -p calimero-runtime --test fugue_wall -- --ignored --nocapture
//!
//! Raise the ceiling if nothing walls in the default range:
//!   FUGUE_WALL_CEILING=40000 cargo test ... -- --ignored --nocapture
//!
//! # It can no longer rot quietly
//!
//! Every failure is classified before it is reported, exactly as in
//! `rga_wall.rs`: `GasExhausted` is the only outcome that produces a number,
//! and anything else is CONTRACT DRIFT and panics naming the method.

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
/// measures a stale binary. Same rule as `rga_wall.rs`.
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

/// Build `fugue-editor` once per test-binary run (cached on disk, rebuilt only
/// when stale) and return its wasm bytes.
fn editor_wasm() -> Vec<u8> {
    let app_dir = workspace_root().join("apps/fugue-editor");
    let wasm_path = app_dir.join("res/fugue_editor.wasm");

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
            "building fugue-editor wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
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

/// What a failed call means. As in `rga_wall`: a measured wall and a broken
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
             takes. Compare the call site below against fugue-editor's current \
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
         \n\
         The in-repo gate for the related property does not depend on this app:\n\
         \x20 cargo test -p storage-cost      (fugue_text_insert_per_char, \
         fugue_text_get_text, fugue_text_char_at)\n\
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
                 against an empty document. That is not a wall — a wall needs data \
                 behind it. Either max_gas has been lowered dramatically or the app now \
                 does unbounded work at n=0."
            )),
        }
    }
}

/// Decode a successful call's return value as a JSON `T`.
fn decode<T: serde::de::DeserializeOwned>(outcome: &Outcome, method: &str) -> T {
    let body = outcome
        .returns
        .as_ref()
        .unwrap_or_else(|_| drift(&format!("{method} failed")))
        .as_ref()
        .unwrap_or_else(|| drift(&format!("{method} returned no value at all")));
    serde_json::from_slice(body)
        .unwrap_or_else(|e| drift(&format!("{method} returned unexpected JSON: {e}")))
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
    let text: String = decode(&read, "get_text");
    if text != "a" {
        drift(&format!(
            "after one insert_text(0, \"a\"), get_text() returned {text:?}, not \"a\". \
             The write is not landing where the read looks, so every number this probe \
             would produce is an artifact."
        ));
    }

    // The positional reads are the whole reason this probe differs from
    // rga_wall, so they are preflighted with the same suspicion as get_text:
    // a char_at that silently returned None would make its "no wall" result
    // meaningless.
    let one = call(
        module,
        &mut storage,
        "char_at",
        &serde_json::json!({"position": 0_usize}),
    );
    expect_ok(&one, "char_at");
    let ch: Option<String> = decode(&one, "char_at");
    if ch.as_deref() != Some("a") {
        drift(&format!(
            "char_at(0) returned {ch:?}, not Some(\"a\") — the positional read is not \
             reading the document this probe is writing."
        ));
    }

    let range = call(
        module,
        &mut storage,
        "text_range",
        &serde_json::json!({"start": 0_usize, "end": 1_usize}),
    );
    expect_ok(&range, "text_range");
    let slice: String = decode(&range, "text_range");
    if slice != "a" {
        drift(&format!(
            "text_range(0, 1) returned {slice:?}, not \"a\" — the range read is not \
             reading the document this probe is writing."
        ));
    }
}

/// Stop here even if nothing has walled, so a genuinely-flat build cannot run
/// forever.
const DEFAULT_CEILING: usize = 20_000;

fn ceiling() -> usize {
    std::env::var("FUGUE_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

/// Characters read by the `text_range` probe — a screenful, not the document.
const RANGE_READ_CHARS: usize = 100;

/// The document ceiling for `FugueText`, measured the way `rga_wall.rs`'s
/// `typing_and_reading_walls` measures RGA's: type one character at a time
/// until a call exhausts gas, probing the reads as the document grows.
#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            fugue-editor app to find where insert_text/get_text/char_at actually \
            exhaust gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (fugue_text_insert_per_char, \
            fugue_text_get_text, fugue_text_char_at)."]
fn typing_and_reading_walls() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    println!("guest:   real fugue-editor app, built from this tree");
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
    // Reads are measured every 100 characters, the same stride rga_wall uses,
    // so the two read curves are sampled identically.
    const READ_PROBE_STRIDE: usize = 100;

    let mut landed = 0_usize;
    let mut write_wall: Option<usize> = None;
    let mut last_read_ok: Option<usize> = None;
    let mut read_wall: Option<usize> = None;
    let mut last_char_at_ok: Option<usize> = None;
    let mut char_at_wall: Option<usize> = None;
    let mut last_range_ok: Option<usize> = None;
    let mut range_wall: Option<usize> = None;

    println!(
        "\n  n        insert_gas       i_reads   ms  | get_text_gas    r_reads  | \
         char_at_gas   c_reads  | range_gas     x_reads"
    );

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

        if !landed.is_multiple_of(READ_PROBE_STRIDE) {
            continue;
        }

        let mut cell = |method: &str,
                        args: serde_json::Value,
                        wall: &mut Option<usize>,
                        last_ok: &mut Option<usize>| {
            if wall.is_some() {
                return "        walled       ".to_owned();
            }
            let read = call(&module, &mut storage, method, &args);
            match &read.returns {
                Ok(_) => {
                    *last_ok = Some(landed);
                    format!("{:>12?}  {:>7}", read.gas_used, read.storage_reads)
                }
                Err(error) => match classify(method, error) {
                    Verdict::Wall { .. } => {
                        *wall = Some(landed);
                        "  WALL (gas exhausted)".to_owned()
                    }
                    Verdict::Drift(detail) => drift(&detail),
                },
            }
        };

        let text_cell = cell(
            "get_text",
            serde_json::json!({}),
            &mut read_wall,
            &mut last_read_ok,
        );
        let char_cell = cell(
            "char_at",
            serde_json::json!({"position": landed / 2}),
            &mut char_at_wall,
            &mut last_char_at_ok,
        );
        let range_cell = cell(
            "text_range",
            serde_json::json!({"start": landed / 2, "end": landed / 2 + RANGE_READ_CHARS}),
            &mut range_wall,
            &mut last_range_ok,
        );

        println!(
            "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1} | {text_cell} | {char_cell} | \
             {range_cell}",
            outcome.gas_used, outcome.storage_reads,
        );
    }

    println!("\n--- result ---");
    println!("characters landed:  {landed}");
    match write_wall {
        Some(n) => println!("write wall (insert_text, one more char): {n}"),
        None => println!("write wall (insert_text, one more char): none below {ceiling}"),
    }
    for (name, last_ok, wall) in [
        ("get_text  ", last_read_ok, read_wall),
        ("char_at   ", last_char_at_ok, char_at_wall),
        ("text_range", last_range_ok, range_wall),
    ] {
        match (last_ok, wall) {
            (Some(ok), Some(bad)) => {
                println!("read wall  ({name}): last OK at {ok}, first exhausted at {bad}")
            }
            (_, Some(bad)) => println!("read wall  ({name}): exhausted by {bad}"),
            (Some(ok), None) => println!("read wall  ({name}): none — still reading at {ok}"),
            (None, None) => println!("read wall  ({name}): never probed"),
        }
    }

    assert!(
        landed > 0,
        "no character was inserted even though preflight succeeded — the failure \
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

/// The MID-DOCUMENT write ceiling: type one character at a time into the
/// MIDDLE of the document (`position: landed / 2`) until a call exhausts gas.
///
/// # Why this exists as a separate probe
///
/// Until it did, nothing here measured mid-document insertion at all, and that
/// blind spot had already produced a misleading result. `typing_and_reading_
/// walls` types at `position: i` — `i` is the count of characters already
/// landed, so every one of its writes is an APPEND at the end, which is
/// precisely the case run-length blocks coalesce. `single_call_paste_wall`
/// pastes into an EMPTY document at position 0, which is one run and no
/// neighbours. So when `FugueText` stopped splitting runs on mid-run insert and
/// the storage-cost table halved (`fugue_text_insert_middle`: 4,083 -> 2,045.5
/// reads/entry at n=2,000), NEITHER wall moved — not because the change did
/// nothing, but because neither wall was looking at the operation it changed.
///
/// Write-only on purpose: the three read walls are already swept by
/// `typing_and_reading_walls`, and probing them again here would multiply the
/// runtime of an already-slow `#[ignore]`d test without asking a new question.
#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            fugue-editor app to find where a MID-DOCUMENT insert_text exhausts \
            gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (fugue_text_insert_middle)."]
fn mid_document_typing_wall() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    println!("guest:   real fugue-editor app, built from this tree");
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
        "no character was inserted even though preflight succeeded — the failure \
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

/// The single-call ceiling of `insert_str`'s BULK path: the largest string that
/// can be pasted into an EMPTY document in ONE `insert_text` call before that
/// one call itself exhausts gas. `rga_wall.rs`'s `single_call_paste_wall`
/// measured 491 characters for `ReplicatedGrowableArray`; this is the same
/// question asked of `FugueText`.
#[test]
#[ignore = "slow: builds the compiled fugue-editor app. Fast to execute once \
            built, unlike the sweep above."]
fn single_call_paste_wall() {
    let wasm = editor_wasm();
    let limits = VMLimits::default();
    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    preflight(&module);

    let paste = |n: usize| -> Result<(), ()> {
        let mut storage = InMemoryStorage::default();
        expect_ok(
            &call(&module, &mut storage, "init", &serde_json::json!({})),
            "init",
        );
        let text: String = std::iter::repeat_n('a', n).collect();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": text}),
        );
        match &outcome.returns {
            Ok(_) => Ok(()),
            Err(error) => match classify("insert_text", error) {
                Verdict::Wall { .. } => Err(()),
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    };

    // `lo` is known to land, `hi` is known to wall. 8,192 is the same seed
    // rga_wall uses: comfortably below the 16 KiB `app::log!` line-length limit
    // that a much larger seed would trip first, misreporting a log overflow as
    // if it bracketed the gas wall.
    let mut lo = 1_usize;
    let mut hi = 8_192;
    if paste(hi).is_ok() {
        drift(&format!(
            "a single insert_text call of {hi} characters into an EMPTY document \
             succeeded. The seeded upper bound for this search is no longer past the \
             wall — raise it in this file."
        ));
    }

    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if paste(mid).is_ok() {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    println!("single-call paste wall: {lo} characters land, {hi} exhausts gas");
    assert!(
        lo >= 10,
        "the single-call paste wall landed at only {lo} characters — investigate \
         before quoting the number, this is far below anything seen so far"
    );
}

/// The discriminator itself runs in CI, even though the two probes above do
/// not. Everything above rests on `classify` telling a measured wall apart
/// from a stale call signature, and that function has no other coverage.
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
            "an application error was classified as a wall — the probe would report \
             a fictional number"
        );
    };
    assert!(
        detail.contains("text"),
        "the drift message must carry the app's own complaint, got: {detail}"
    );

    let Verdict::Drift(detail) = classify(
        "char_at",
        &FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            name: "char_at".to_owned(),
        }),
    ) else {
        panic!("a renamed method was classified as a wall");
    };
    assert!(
        detail.contains("char_at"),
        "the drift message must name the method, got: {detail}"
    );
}
