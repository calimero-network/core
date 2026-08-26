//! Every workload's declared `tolerance_pct` must match what it actually does.
//!
//! The snapshot gate compares measured row counts against a committed file. It
//! is only meaningful if the measurement reproduces, and only useful if the
//! band it allows is no wider than it has to be. Both halves rot silently:
//! a tolerance that is too tight makes CI flaky (and someone widens it), a
//! tolerance that is too wide makes the gate blind (and nobody notices).
//!
//! So the tolerance is re-derived here from live measurements rather than
//! trusted. `RUNS` is small enough to stay quick and large enough that a
//! genuinely varying count shows up.

use std::collections::BTreeMap;

use storage_cost::measure;
use storage_cost::workloads::all;

/// Repeats per workload. Seven was enough to separate the one varying workload
/// from the twenty-three exact ones when the tolerances were first derived.
const RUNS: usize = 7;

/// No workload may claim more slack than this. A cost allowed to move by more
/// than a quarter is not being gated on its constant any more, only on its
/// order of magnitude — and that job belongs to `flat_curve.rs`.
const MAX_DECLARED_TOLERANCE_PCT: u32 = 25;

#[test]
fn declared_tolerances_bound_the_observed_spread() {
    let mut failures = Vec::new();

    for workload in all() {
        assert!(
            workload.tolerance_pct <= MAX_DECLARED_TOLERANCE_PCT,
            "{}/{} declares tolerance_pct={} — above the {MAX_DECLARED_TOLERANCE_PCT} \
             cap. A band that wide is not a cost gate.",
            workload.name,
            workload.n,
            workload.tolerance_pct
        );

        let mut counts = BTreeMap::<&str, Vec<u64>>::new();
        for _ in 0..RUNS {
            let (_, costs) = measure(|| (workload.run)(workload.n));
            counts.entry("rows_read").or_default().push(costs.rows_read);
            counts
                .entry("rows_written")
                .or_default()
                .push(costs.rows_written);
            counts
                .entry("rows_removed")
                .or_default()
                .push(costs.rows_removed);
        }

        for (metric, values) in counts {
            let lo = *values.iter().min().expect("RUNS > 0");
            let hi = *values.iter().max().expect("RUNS > 0");
            if lo == 0 {
                continue;
            }
            let spread_pct = (hi - lo) as f64 * 100.0 / lo as f64;
            if spread_pct > f64::from(workload.tolerance_pct) {
                failures.push(format!(
                    "{}/{} {metric}: varied {lo}..{hi} over {RUNS} runs ({spread_pct:.1}%) \
                     but declares tolerance_pct={}. Either the measurement became \
                     nondeterministic — find out why before widening anything — or the \
                     declared tolerance in workloads.rs is stale.",
                    workload.name, workload.n, workload.tolerance_pct
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "declared tolerances do not bound what was measured:\n  {}",
        failures.join("\n  ")
    );
}
