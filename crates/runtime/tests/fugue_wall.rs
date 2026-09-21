//! Where a real `FugueText` document walls: writes, then reads.
//!
//!   cargo test -p calimero-runtime --test fugue_wall -- --ignored --nocapture

use std::time::Instant;

use calimero_runtime::logic::VMLimits;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{call, guest_wasm, Probe, Verdict};

const PROBE: Probe = Probe {
    app: "fugue-editor",
    gate: "\n\
           The in-repo gate for the related property does not depend on this app:\n\
           \x20 cargo test -p storage-cost      (fugue_text_insert_per_char, \
           fugue_text_char_at)\n\
           \x20 ./scripts/check-storage-cost.sh\n",
};

fn editor_wasm() -> Vec<u8> {
    guest_wasm(PROBE.app)
}

fn preflight(module: &calimero_runtime::Module) {
    let mut storage = InMemoryStorage::default();
    PROBE.expect_ok(
        &call(module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    PROBE.expect_ok(
        &call(
            module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": "a"}),
        ),
        "insert_text",
    );

    let read = call(module, &mut storage, "get_text", &serde_json::json!({}));
    PROBE.expect_ok(&read, "get_text");
    let text: String = PROBE.decode(&read, "get_text");
    if text != "a" {
        PROBE.drift(&format!(
            "after one insert_text(0, \"a\"), get_text() returned {text:?}, not \"a\". \
             The write is not landing where the read looks, so every number this probe \
             would produce is an artifact."
        ));
    }

    let one = call(
        module,
        &mut storage,
        "char_at",
        &serde_json::json!({"position": 0_usize}),
    );
    PROBE.expect_ok(&one, "char_at");
    let ch: Option<String> = PROBE.decode(&one, "char_at");
    if ch.as_deref() != Some("a") {
        PROBE.drift(&format!(
            "char_at(0) returned {ch:?}, not Some(\"a\") - the positional read is not \
             reading the document this probe is writing."
        ));
    }

    let range = call(
        module,
        &mut storage,
        "text_range",
        &serde_json::json!({"start": 0_usize, "end": 1_usize}),
    );
    PROBE.expect_ok(&range, "text_range");
    let slice: String = PROBE.decode(&range, "text_range");
    if slice != "a" {
        PROBE.drift(&format!(
            "text_range(0, 1) returned {slice:?}, not \"a\" - the range read is not \
             reading the document this probe is writing."
        ));
    }
}

const DEFAULT_CEILING: usize = 20_000;

fn ceiling() -> usize {
    std::env::var("FUGUE_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

const RANGE_READ_CHARS: usize = 100;

#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            fugue-editor app to find where insert_text/get_text/char_at actually \
            exhaust gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (fugue_text_insert_per_char, \
            fugue_text_char_at)."]
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
    PROBE.expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );

    let ceiling = ceiling();
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
            match PROBE.classify("insert_text", error) {
                Verdict::Wall { limit } => {
                    println!("\nWRITE WALL at {landed} (gas limit {limit})");
                    write_wall = Some(landed);
                    break;
                }
                Verdict::Drift(detail) => PROBE.drift(&format!(
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
                Err(error) => match PROBE.classify(method, error) {
                    Verdict::Wall { .. } => {
                        *wall = Some(landed);
                        "  WALL (gas exhausted)".to_owned()
                    }
                    Verdict::Drift(detail) => PROBE.drift(&detail),
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
            (Some(ok), None) => println!("read wall  ({name}): none, still reading at {ok}"),
            (None, None) => println!("read wall  ({name}): never probed"),
        }
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

#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            fugue-editor app to find where a MID-DOCUMENT insert_text exhausts \
            gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (fugue_text_insert_middle)."]
fn mid_document_typing_wall() {
    wall_harness::mid_document_typing_wall(&PROBE, &editor_wasm(), ceiling(), preflight);
}

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

    let paste = |n: usize| -> Result<Option<u64>, ()> {
        let mut storage = InMemoryStorage::default();
        PROBE.expect_ok(
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
            Ok(_) => Ok(outcome.gas_used),
            Err(error) => match PROBE.classify("insert_text", error) {
                Verdict::Wall { .. } => Err(()),
                Verdict::Drift(detail) => PROBE.drift(&detail),
            },
        }
    };

    // Binary search: `lo` lands, `hi` walls; `hi` cannot pass the 16 KiB `app::log!` limit.
    let mut lo = 1_usize;
    let mut hi = 16_000;
    if let Ok(gas) = paste(hi) {
        println!(
            "single-call paste wall: not reachable through fugue-editor; {hi} characters \
             land ({gas:?} gas) and the app's log-line limit caps a larger paste"
        );
        return;
    }

    let mut lo_gas: Option<u64> = None;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        match paste(mid) {
            Ok(gas) => {
                lo = mid;
                lo_gas = gas;
            }
            Err(()) => hi = mid,
        }
    }

    println!("single-call paste wall: {lo} characters land ({lo_gas:?} gas), {hi} exhausts gas");
    assert!(
        lo >= 10,
        "the single-call paste wall landed at only {lo} characters - investigate \
         before quoting the number, this is far below anything seen so far"
    );
}
