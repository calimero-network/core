//! Where a real `ReplicatedGrowableArray` document walls: writes, then reads.
//!
//!   cargo test -p calimero-runtime --test rga_wall -- --ignored --nocapture

use std::time::Instant;

use calimero_runtime::logic::VMLimits;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{call, guest_wasm, Probe, Verdict};

const PROBE: Probe = Probe {
    app: "collaborative-editor",
    gate: "\n\
           The in-repo gate for the related property does not depend on this app:\n\
           \x20 cargo test -p storage-cost      (rga_insert_per_char, rga_get_nth)\n\
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
    // `get_text` returns a bare JSON string, not an "output" envelope.
    let text: String = PROBE.decode(&read, "get_text");
    if text != "a" {
        PROBE.drift(&format!(
            "after one insert_text(0, \"a\"), get_text() returned {text:?}, not \"a\". \
             The write is not landing where the read looks, so every number this probe \
             would produce is an artifact."
        ));
    }
}

const DEFAULT_CEILING: usize = 20_000;

fn ceiling() -> usize {
    std::env::var("RGA_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

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
                Err(error) => match PROBE.classify("get_text", error) {
                    Verdict::Wall { .. } => {
                        read_wall = Some(landed);
                        println!(
                            "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1} | READ WALL (gas exhausted)",
                            outcome.gas_used, outcome.storage_reads,
                        );
                    }
                    Verdict::Drift(detail) => PROBE.drift(&detail),
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

#[test]
#[ignore = "slow: executes thousands of real WASM calls against the compiled \
            collaborative-editor app to find where a MID-DOCUMENT insert_text \
            exhausts gas. The in-repo gate for the same underlying property is \
            `cargo test -p storage-cost` (rga_insert_per_char)."]
fn mid_document_typing_wall() {
    wall_harness::mid_document_typing_wall(&PROBE, &editor_wasm(), ceiling(), preflight);
}

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

    // Binary search: `lo` lands, `hi` walls.
    let mut lo = 1_usize;
    // Below the 16 KiB `app::log!` limit, which a larger seed would trip first.
    let mut hi = 8_192;
    let mut hi_confirmed = false;
    while !hi_confirmed {
        let mut storage = InMemoryStorage::default();
        PROBE.expect_ok(
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
            Ok(_) => PROBE.drift(&format!(
                "a single insert_text call of {hi} characters into an EMPTY document \
                 succeeded. The seeded upper bound for this search is no longer past \
                 the wall; raise it in this file."
            )),
            Err(error) => match PROBE.classify("insert_text", error) {
                Verdict::Wall { .. } => hi_confirmed = true,
                Verdict::Drift(detail) => PROBE.drift(&detail),
            },
        }
    }

    let mut lo_gas: Option<u64> = None;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let mut storage = InMemoryStorage::default();
        PROBE.expect_ok(
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
            Err(error) => match PROBE.classify("insert_text", error) {
                Verdict::Wall { .. } => hi = mid,
                Verdict::Drift(detail) => PROBE.drift(&detail),
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
