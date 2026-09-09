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
    // One declared `tolerance_pct` covers every `n` a workload is run at
    // (see `Workload::tolerance_pct`'s doc comment), and observed spread
    // shrinks sharply as `n` grows (`vector_get_nth` measures ~10x more
    // spread at n=10 than at n=10000 — a bigger trie averages the same
    // per-bucket randomness over more buckets). So the too-wide check below
    // is evaluated once per workload NAME, against the worst (largest)
    // spread seen at any of its sizes — the smallest `n` is what the
    // declared tolerance actually has to cover, and it is what any of these
    // three comments should be read as promising.
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

    // The other direction: a declared tolerance far wider than the worst
    // spread `RUNS` samples actually showed, at any size, is a blind gate —
    // it will pass a real regression that lands inside the unused slack.
    //
    // The worst-spread statistic is itself noisy: it is a max-of-order-
    // statistics over only `RUNS` samples on an integer row count already
    // sitting around ~40 at n=10, so a difference of a couple of rows swings
    // it by several points. Repeated live measurements of `vector_get_nth`
    // while this rule was chosen ranged as low as ~4% and as high as ~10.5%,
    // all for the exact same code — so the rule has to tolerate that much
    // run-to-run swing in the denominator without flapping on CI. The rule
    // below is `3x` the worst observed spread plus a flat 8 percentage
    // points: generous enough that a run landing as low as ~3.3% still
    // clears an 18% declaration, while still refusing to let a declaration
    // coast on the `MAX_DECLARED_TOLERANCE_PCT` cap for a workload that
    // would only justify a fraction of it (to justify the 25% cap under this
    // rule, a workload now needs at least ~5.7% real observed spread, not
    // "some spread, so round up to the cap").
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
