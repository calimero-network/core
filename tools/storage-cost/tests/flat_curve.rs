//! Tier 1b: assert the SHAPE of each cost curve, not its constant.
//!
//! A per-operation cost that does not grow with collection size is the property
//! that keeps a collection usable forever. Asserting it directly is
//! machine-independent, immune to constant-factor drift, and runs in seconds —
//! the four regressions in core#3602 were all violations of it, including one
//! caused by a log line calling `enumerate()` instead of counting.
//!
//! Deliberately NOT a criterion benchmark: this is a correctness property.
//!
//! # Both directions
//!
//! `KnownLinearInN` workloads are asserted to STILL be linear. A cost that
//! silently stops being linear is good news, but news — the marker, the
//! snapshot and the comment explaining the wall all have to move with it. An
//! improvement that nobody notices is how a stale "known issue" outlives the
//! issue.
//!
//! # Relationship to `crates/runtime/tests/cost_is_flat.rs`
//!
//! That test asks a different question one layer down: does a WASM guest doing
//! a FIXED number of host calls get charged more gas because the STORE grew? It
//! uses a synthetic WAT guest and never touches `calimero-storage`.
//!
//! This one asks whether a `calimero-storage` COLLECTION operation costs more
//! per entry as the collection grows. Neither subsumes the other: a flat host
//! layer with an O(n) collection — exactly where this tree stands, see
//! `vector_get_nth` — passes there and is caught here.

use std::collections::BTreeMap;

use storage_cost::workloads::{all, CostShape, SIZES};
use storage_cost::{measure, Costs};

/// Cost may grow by at most this factor between the smallest and largest
/// collection size before it stops being "bounded with a plateau" and starts
/// being "linear in n".
///
/// Ratio rather than equality because a build of `n` entries legitimately
/// touches a little more per entry as interior index nodes fill.
const MAX_GROWTH: f64 = 2.0;

/// A `KnownLinearInN` read must cost at least `n / LINEAR_FLOOR_DIVISOR` rows
/// at the largest measured size.
///
/// Stated as an absolute floor rather than a growth ratio on purpose. The trie
/// packs better as it deepens, so reads/entry legitimately *falls* with `n`
/// (4.0 at n=10, 1.3 at n=10,000) while the cost stays linear — a ratio test
/// would confuse that with a fix. A sublinear replacement would land three
/// orders of magnitude below this floor, so the band is wide and the verdict is
/// unambiguous.
const LINEAR_FLOOR_DIVISOR: f64 = 100.0;

/// …and at most `n * LINEAR_CEILING_FACTOR`. Past that it is superlinear.
const LINEAR_CEILING_FACTOR: f64 = 4.0;

const SMALLEST: usize = SIZES[0];
const LARGEST: usize = SIZES[SIZES.len() - 1];

/// Measured cost of every workload of `shape`, as `name -> n -> cost`.
///
/// `FlatPerEntry` costs are divided by `n`, because the workload deliberately
/// does `n` operations. Point workloads are not: they perform exactly one
/// operation, and its absolute cost is the thing under test.
fn series(
    shape: CostShape,
    metric: fn(Costs) -> u64,
) -> BTreeMap<&'static str, BTreeMap<usize, f64>> {
    let mut by_name: BTreeMap<&'static str, BTreeMap<usize, f64>> = BTreeMap::new();
    for workload in all().into_iter().filter(|w| w.shape == shape) {
        let (_, costs) = measure(|| (workload.run)(workload.n));
        let divisor = if shape == CostShape::FlatPerEntry {
            workload.n as f64
        } else {
            1.0
        };
        let _ignored = by_name
            .entry(workload.name)
            .or_default()
            .insert(workload.n, metric(costs) as f64 / divisor);
    }
    assert!(
        !by_name.is_empty(),
        "no {shape:?} workloads were measured — the registry filter is wrong and this \
         test is asserting nothing"
    );
    by_name
}

fn report(failures: Vec<String>, headline: &str) {
    assert!(
        failures.is_empty(),
        "{headline}:\n  {}",
        failures.join("\n  ")
    );
}

fn assert_bounded(unit: &str, shape: CostShape, metric: fn(Costs) -> u64) {
    let mut failures = Vec::new();
    for (name, points) in &series(shape, metric) {
        let (small, large) = (points[&SMALLEST], points[&LARGEST]);
        if small > 0.0 && large > small * MAX_GROWTH {
            failures.push(format!(
                "{name}: {unit} grew {small:.1} (n={SMALLEST}) -> {large:.1} \
                 (n={LARGEST}), {:.1}x — budget is {MAX_GROWTH}x",
                large / small
            ));
        }
    }
    report(
        failures,
        &format!("{shape:?} workloads: {unit} cost grows with collection size"),
    );
}

#[test]
fn build_cost_per_entry_does_not_grow_with_collection_size() {
    assert_bounded("writes/entry", CostShape::FlatPerEntry, |c| c.rows_written);
    assert_bounded("reads/entry", CostShape::FlatPerEntry, |c| c.rows_read);
}

/// Reads are the assertion that matters most: they are invisible to gas
/// accounting (no read counter, no read limit in the VM), so an O(n) read
/// pattern shows up as neither a gas charge nor a wall-clock signal until a
/// call dies outright.
#[test]
fn point_operation_cost_does_not_grow_with_collection_size() {
    assert_bounded("reads/call", CostShape::ConstantPerCall, |c| c.rows_read);
    assert_bounded("writes/call", CostShape::ConstantPerCall, |c| {
        c.rows_written
    });
}

/// The ratchet on costs we know are O(n) and have chosen not to fix yet.
///
/// Fails if one gets worse than linear, and fails — differently, and on
/// purpose — if one stops being linear, so a fix cannot land while the
/// documentation still describes the wall.
#[test]
fn known_linear_costs_are_still_exactly_linear() {
    let floor = LARGEST as f64 / LINEAR_FLOOR_DIVISOR;
    let ceiling = LARGEST as f64 * LINEAR_CEILING_FACTOR;
    let mut failures = Vec::new();

    for (name, points) in &series(CostShape::KnownLinearInN, |c| c.rows_read) {
        let large = points[&LARGEST];

        if large < floor {
            failures.push(format!(
                "{name}: reads/call at n={LARGEST} is {large:.0}, below the {floor:.0} \
                 floor that marks a linear cost — this appears to have been FIXED. That \
                 is good news, and it must be recorded: move it to \
                 CostShape::ConstantPerCall, regenerate \
                 tools/storage-cost/storage-costs.json, and update the wall write-up in \
                 docs/superpowers/2026-08-26-chat-read-wall.md"
            ));
        } else if large > ceiling {
            failures.push(format!(
                "{name}: reads/call at n={LARGEST} is {large:.0}, above the \
                 {ceiling:.0} ceiling — worse than linear, i.e. a regression stacked on \
                 top of a known-bad cost"
            ));
        }
    }

    report(failures, "known-linear costs no longer match their marker");
}
