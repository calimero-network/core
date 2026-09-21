//! Whether the single-call write wall is an RGA defect or a storage-layer property.
//!
//!   cargo test -p calimero-runtime --test bulk_write_wall -- --ignored --nocapture

use calimero_runtime::logic::VMLimits;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{call, guest_wasm, Probe, Verdict};

const PROBE: Probe = Probe {
    app: "bulk-write-bench",
    gate: "",
};

fn bench_wasm() -> Vec<u8> {
    guest_wasm(PROBE.app)
}

fn preflight(module: &calimero_runtime::Module, method: &str) {
    let mut storage = InMemoryStorage::default();
    PROBE.expect_ok(
        &call(module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    PROBE.expect_ok(
        &call(
            module,
            &mut storage,
            method,
            &serde_json::json!({"n": 1_u32}),
        ),
        method,
    );
}

/// Returns `(largest_landed, gas, storage_reads, storage_writes, first_exhausted)`.
fn find_wall(module: &calimero_runtime::Module, method: &str) -> (usize, u64, u64, u64, usize) {
    preflight(module, method);

    // Binary search: `lo` lands, `hi` walls; the bracket is checked once, never widened.
    let mut lo = 1_usize;
    let mut hi = 8_192_usize;
    {
        let mut storage = InMemoryStorage::default();
        PROBE.expect_ok(
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
            Ok(_) => PROBE.drift(&format!(
                "a single {method} call with n={hi} into an EMPTY collection succeeded. \
                 The seeded upper bound for this search is no longer past the wall; \
                 raise it in this file."
            )),
            Err(error) => match PROBE.classify(method, error) {
                Verdict::Wall { .. } => {}
                Verdict::Drift(detail) => PROBE.drift(&detail),
            },
        }
    }

    let mut lo_gas = 0_u64;
    let mut lo_reads = 0_u64;
    let mut lo_writes = 0_u64;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let mut storage = InMemoryStorage::default();
        PROBE.expect_ok(
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
            Err(error) => match PROBE.classify(method, error) {
                Verdict::Wall { .. } => hi = mid,
                Verdict::Drift(detail) => PROBE.drift(&detail),
            },
        }
    }

    (lo, lo_gas, lo_reads, lo_writes, hi)
}

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
            "{label} walled after only {lo} entries - investigate before quoting the \
             number, this is far below anything expected"
        );
        assert_eq!(
            *hi,
            lo + 1,
            "{label}: binary search did not converge to adjacent lo/hi"
        );
    }
}
