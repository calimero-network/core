//! Where a real `ReplicatedGrowableArray` document stops being writable, and
//! separately where it stops being readable at all.
//!
//! # Why a real contract here, unlike `cost_is_flat`
//!
//! `cost_is_flat` deliberately uses a synthetic guest so that any growth it
//! measures is unambiguously a defect in the storage layer. This probe wants
//! the opposite: the number a user of a real editor actually hits. That
//! requires the actual app — `apps/collaborative-editor` — because its
//! `insert_text` maps a single keystroke onto `ReplicatedGrowableArray::
//! insert_str` with a one-character string, which is the per-keystroke access
//! pattern an editor really uses, not the raw `ReplicatedGrowableArray::insert`
//! the in-repo `rga_insert_per_char` workload (`tools/storage-cost`) calls
//! directly.
//!
//! # No cross-repo dependency, unlike `chat_wall`
//!
//! `chat_wall.rs` drives a contract that lives in a sibling repo and is
//! `#[ignore]`d partly because of that fragility. `collaborative-editor` is
//! in this workspace, so this probe has none of that: it builds the app with
//! `cargo mero build` against whatever is checked out right here. It is still
//! `#[ignore]`d, for the other reason `chat_wall` is too — the sweep below is
//! slow, because reaching a wall this large means executing thousands of real
//! WASM calls, each more expensive than the last.
//!
//! # What "the wall" means
//!
//! Gas is charged per executed wasm operator, and NOTHING else — there is no
//! read counter or read limit in the VM. `ReplicatedGrowableArray::insert_str`
//! re-derives the character's position by linearising the WHOLE document on
//! every call, an `O(current length)` cost paid once per call regardless of
//! how much new text that call inserts. So a `send`-a-single-keystroke loop is
//! `O(n)` per call and `O(n^2)` in total, and the read count that linearisation
//! performs shows up in gas as the cost of borsh-decoding what those reads
//! returned. Eventually one `insert_text` call exceeds `max_gas` and traps —
//! and every later call traps too, because the cost only grows from there. The
//! document is permanently unwritable from that point (core#3602 was exactly
//! this shape, in a different collection). The write wall is the length of the
//! document after the last character that landed.
//!
//! `get_text()` performs the SAME linearisation to answer a read — it is the
//! only read `ReplicatedGrowableArray` has — so it has its own wall, measured
//! separately: the document length at which `get_text()` itself can no longer
//! complete inside `max_gas`. A document that can still accept writes but can
//! no longer be opened is just as dead as one that cannot be written at all,
//! and the two walls are not assumed to be the same distance out — that is
//! exactly what this probe checks.
//!
//! # Measured results (2026-08-31, against this tree)
//!
//! * **Write wall (typing, one keystroke at a time)** — EXTRAPOLATED. Largest
//!   executed point: a single `insert_text` call at document length 8,400
//!   characters costs 696,466,208 gas (70% of the 1,000,000,000 ceiling); at
//!   1,000 characters it costs 102,870,342. That pair fits a line
//!   (slope ≈80,216 gas/character, intercept ≈22.7M) that crosses
//!   1,000,000,000 gas at **≈12,190 characters**. No call in the executed
//!   range (100 through 8,400) actually returned `GasExhausted` — the sweep
//!   was stopped short of the wall to keep this probe's `#[ignore]`d run
//!   inside a few minutes; see `typing_and_reading_walls` for how to push the
//!   ceiling higher and turn this into an executed number.
//!
//! * **Bulk `insert_str`, one call, empty document** — EXECUTED, by binary
//!   search: a single call pasting 491 characters into an empty document
//!   lands (998,651,983 gas); 492 characters returns `GasExhausted`. Cost is
//!   almost exactly linear with zero fixed overhead — ≈2,038,000 gas per
//!   NEW character in that one call (811,689,272 gas / 400 characters ≈
//!   2,029,223 gas/char; 997,206,303 gas / 490 characters ≈ 2,035,115
//!   gas/char — the same slope within noise) — because the call pays a
//!   `48 reads/char` flat-insert cost with a
//!   MUCH higher gas-per-read than the per-keystroke path below (see the
//!   reconciliation note in `typing_and_reading_walls`'s doc comment).
//!
//!   This is not the escape hatch it looks like. A single `insert_str` call
//!   is capped at ~491 NEW characters no matter how empty the document is —
//!   so any document longer than that has to be built from more than one
//!   call regardless, and every call after the first still pays the SAME
//!   `O(current document length)` linearisation the per-keystroke path pays.
//!   Batching buys you fewer, chunkier calls; it does not buy you a
//!   different asymptotic wall. "RGA is fine if you never call it the way an
//!   editor would" is not what this measures — there is no way to call it
//!   that stays flat past a few hundred characters per call.
//!
//! * **Read wall (`get_text`)** — EXTRAPOLATED, from the SAME sweep that
//!   produced the write wall (one `get_text` call every 100 characters, on
//!   the same growing document). Largest executed point: at 8,400 characters
//!   `get_text` costs 692,379,273 gas; at 1,000 characters it costs
//!   98,367,475. That line crosses 1,000,000,000 gas at **≈12,230
//!   characters** — inside a few hundred characters of the write wall above,
//!   not far from it. An earlier estimate put the read wall near 9,200
//!   characters by extrapolating from a single `get_text` call against a
//!   document built with ONE bulk `insert_str` (400 characters, 43,554,113
//!   gas). That single point was a weaker basis than the swept curve here:
//!   it could not distinguish an early, not-yet-converged per-character rate
//!   from the true asymptotic slope, and it measured `get_text` against a
//!   document built the one way `insert_str` cannot build past ~491
//!   characters. The 18-point swept fit above supersedes it: write and read
//!   die within measurement noise of each other, not "far apart".
//!
//! # It can no longer rot quietly
//!
//! Every failure is classified before it is reported, mirroring `chat_wall`'s
//! discipline:
//!
//! * `GasExhausted` — a real wall. The only outcome that produces a number.
//! * anything else — CONTRACT DRIFT. Method renamed, arguments changed. The
//!   probe panics naming the method and the error, and never reports a wall,
//!   because a wall it did not measure is worse than no wall.
//!
//! A preflight runs one `insert_text` and one `get_text` on an empty document
//! before the sweep starts, so drift is reported in seconds.
//!
//! # Running it
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

/// Newest mtime across the app's build inputs. Mirrors `tracing_logs.rs`'s
/// fixture: rebuild whenever any `*.rs` under `src/`, `Cargo.toml`, or
/// `build.rs` is newer than the last build, so this probe never silently
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
    /// The guest did not get far enough to cost anything meaningful — the
    /// method is gone, the arguments no longer deserialise. This is not a
    /// result, and must never be presented as one.
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
         ==================== CONTRACT DRIFT — NOT A STORAGE RESULT ====================\n\
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
                 against an empty document. That is not a wall — a wall needs data \
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
    // `get_text` returns the document as a bare JSON string (not wrapped in an
    // "output" envelope) — confirmed against the compiled app, not assumed.
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

/// Stop here even if nothing has walled, so a genuinely-flat build cannot run
/// forever.
const DEFAULT_CEILING: usize = 20_000;

fn ceiling() -> usize {
    std::env::var("RGA_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

/// Reconciling `outcome.storage_reads` here against `rga_insert_per_char`'s
/// committed `n + 47` reads/entry (`tools/storage-cost`, `storage-costs.json`):
/// they are NOT the same statistic and are not expected to match.
///
/// 1. **Average over a whole build, vs. one late call.** `rga_insert_per_char`
///    resets nothing between calls and reports `rows_read` for the WHOLE
///    build of `n` calls divided by `n` — the historical AVERAGE, blending
///    cheap early calls with expensive late ones. This probe's
///    `i_reads`/`r_reads` columns are `Outcome::storage_reads` for ONE call
///    at a given document length, taken fresh (a new `Module::run`, whose
///    counters start at zero — see `logic.rs`'s doc comment on
///    `storage_reads`, "so far" meaning "in this execution"). Since the
///    per-call cost is roughly linear in document length, the LAST call of a
///    build of `n` costs roughly double the AVERAGE over that same build —
///    accounting for part, but not all, of the gap below.
/// 2. **Different scope.** `rga_insert_per_char` calls
///    `ReplicatedGrowableArray::insert` directly, in-process, through
///    `calimero-storage`'s own counting `RuntimeEnv` — no WASM, no SDK, no
///    other collection in the picture. This probe counts every
///    `env::storage_read` HOST-IMPORT call the COMPILED APP makes during one
///    `insert_text`/`get_text` invocation, which additionally includes
///    `#[app::state]`'s root-state fetch/commit (touching `edit_count` and
///    `metadata` alongside `document`, since all three live in the same
///    struct), `Counter::increment()`'s own read(s), and any register/ABI
///    marshaling the SDK performs. This is the real end-to-end count, not an
///    isolated one — the same distinction `chat_wall.rs` draws between a
///    synthetic guest and a real contract.
///
/// At document length 7,800 this probe measured 26,857 reads for one
/// `insert_text` call; `n + 47` would predict ~7,847 for the SAME `n` under
/// `rga_insert_per_char`'s accounting. Point (1) alone would only close
/// roughly half that gap (an average-vs-last-call factor of ~2x); the
/// remainder is attributed to point (2), the larger call scope, but has not
/// been decomposed further — doing so would need a guest stripped of the
/// `Counter`/`metadata` fields and the SDK's ABI marshaling to isolate the
/// RGA-only cost inside a real compiled app, which is a follow-up
/// measurement, not something resolved by the data this probe collected.
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

        if read_wall.is_none() && landed % READ_PROBE_STRIDE == 0 {
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

/// The single-call ceiling of `insert_str`'s BULK path: the largest string
/// that can be pasted into an EMPTY document in ONE `insert_text` call before
/// that one call itself exhausts gas.
///
/// This is a different question from the write wall above. The write wall is
/// "how long can the document GET, one keystroke at a time" — the linearise
/// cost dominates and grows with the document's EXISTING length. This is "how
/// much NEW text can ONE call add, no matter how empty the document is" — the
/// linearise cost is paid once (on an empty document, near zero) and the flat
/// per-character insert cost dominates instead. Reporting both is what turns
/// "RGA is broken" into "RGA is broken two different ways depending on how you
/// drive it" — see the module docs and the report this probe feeds.
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

    // Binary search: `lo` is known to land, `hi` is known to wall. Seed from a
    // pair well outside the true wall so a shift in the app's cost model still
    // brackets it correctly.
    let mut lo = 1_usize;
    let mut hi = 8_192; // comfortably above the observed wall (~500) and safely
                        // below the 16 KiB `app::log!` line-length limit that a much
                        // larger seed would trip first, misreporting a log overflow
                        // as if it bracketed the gas wall
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
                 the wall — raise it in this file."
            )),
            Err(error) => match classify("insert_text", error) {
                Verdict::Wall { .. } => hi_confirmed = true,
                Verdict::Drift(detail) => drift(&detail),
            },
        }
    }

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
            Ok(_) => lo = mid,
            Err(error) => match classify("insert_text", error) {
                Verdict::Wall { .. } => hi = mid,
                Verdict::Drift(detail) => drift(&detail),
            },
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
