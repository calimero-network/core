//! Re-derives every workload's declared `tolerance_pct` from live runs rather
//! than trusting it. Both halves rot silently: too tight makes the gate flaky,
//! too wide makes it blind.
//!
//! ```text
//! cargo test -p storage-cost --release --test reproducible -- --include-ignored
//! ```

use std::collections::BTreeMap;

use storage_cost::measure;
use storage_cost::workloads::all;

const RUNS: usize = 7; // enough to separate the varying workloads from the exact ones
const MAX_DECLARED_TOLERANCE_PCT: u32 = 25; // past a quarter, only the order of magnitude is gated

#[ignore = "minutes of work; run by the release storage-cost CI job via --include-ignored"]
#[test]
fn declared_tolerances_bound_the_observed_spread() {
    let mut failures = Vec::new();
    // One declared tolerance covers every `n`, and spread shrinks sharply as
    // `n` grows, so the too-wide check below runs once per workload NAME
    // against the worst spread seen at any of its sizes.
    let mut worst_spread_pct = BTreeMap::<&str, f64>::new();
    let mut tolerance_pct = BTreeMap::<&str, u32>::new();

    for workload in all() {
        assert!(
            workload.tolerance_pct <= MAX_DECLARED_TOLERANCE_PCT,
            "{}/{} declares tolerance_pct={} — above the {MAX_DECLARED_TOLERANCE_PCT} \
             cap. A band that wide is not a cost gate.",
            workload.name,
            workload.n,
            workload.tolerance_pct
        );
        tolerance_pct.insert(workload.name, workload.tolerance_pct);

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

            let entry = worst_spread_pct.entry(workload.name).or_insert(0.0);
            if spread_pct > *entry {
                *entry = spread_pct;
            }
        }
    }

    // The other direction: a band far wider than the observed spread is a
    // blind gate. The bound is 3x the worst spread plus 8 points, because the
    // spread statistic is itself noisy over only `RUNS` integer samples.
    for (name, worst_spread_pct) in worst_spread_pct {
        let declared = tolerance_pct[name];
        let max_reasonable_tolerance_pct = worst_spread_pct * 3.0 + 8.0;
        if f64::from(declared) > max_reasonable_tolerance_pct {
            failures.push(format!(
                "{name}: declares tolerance_pct={declared} but the worst spread observed \
                 across all sizes was {worst_spread_pct:.1}% — a band that wide (more than \
                 3x the worst observed spread plus 8 points, i.e. over \
                 {max_reasonable_tolerance_pct:.1}%) would let a real regression pass \
                 silently. Tighten the declared tolerance in workloads.rs to match what is \
                 actually observed."
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "declared tolerances do not bound what was measured:\n  {}",
        failures.join("\n  ")
    );
}
