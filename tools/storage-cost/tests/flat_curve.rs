//! Asserts the SHAPE of each cost curve rather than its constant, which is
//! machine-independent and immune to constant-factor drift.
//!
//! ```text
//! cargo test -p storage-cost --release --test flat_curve
//! ```
//!
//! Every shape is checked in both directions: a `KnownLinearInN` cost that
//! silently stops being linear fails too, so a fix cannot land while the
//! marker and the snapshot still describe the wall.
//!
//! `crates/runtime/tests/cost_is_flat.rs` asks a different question one layer
//! down - whether a guest doing a fixed number of host calls is charged more
//! gas as the store grows - and neither test subsumes the other.

use std::collections::BTreeMap;

use storage_cost::workloads::{all, CostShape, QUADRATIC_SIZES};
use storage_cost::{measure, Costs};

const MAX_GROWTH: f64 = 2.0; // a ratio, not equality: interior index nodes fill as n grows

/// A `KnownLinearInN` read must cost at least `n / LINEAR_FLOOR_DIVISOR` rows
/// at the largest size. An absolute floor, not a ratio: the trie packs better
/// as it deepens, so reads/entry falls with `n` while staying linear.
const LINEAR_FLOOR_DIVISOR: f64 = 200.0;

const LINEAR_CEILING_FACTOR: f64 = 4.0; // above n * this, the cost is superlinear

fn span(points: &BTreeMap<usize, f64>) -> (usize, usize) {
    let (&smallest, _) = points
        .first_key_value()
        .expect("a measured workload has at least one point");
    let (&largest, _) = points
        .last_key_value()
        .expect("a measured workload has at least one point");
    (smallest, largest)
}

/// Below `n / this` reads per entry, a build has stopped being quadratic.
/// Calibrated against the two live cases: `rga_insert_per_char` measures 2047
/// reads/entry at `n=2_000`, and the `insert_str` fix would measure 48.
const QUADRATIC_FLOOR_DIVISOR: f64 = 5.0;

const QUADRATIC_CEILING_FACTOR: f64 = 4.0; // above n * this, worse than quadratic

const QUADRATIC_LARGEST: usize = QUADRATIC_SIZES[QUADRATIC_SIZES.len() - 1];

/// Measured cost of every workload of `shape`, as `name -> n -> cost`. Build
/// shapes are divided by `n` because what is under test is the average cost of
/// one operation; point shapes do one operation, so their total is the cost.
fn series(
    shape: CostShape,
    metric: fn(Costs) -> u64,
) -> BTreeMap<&'static str, BTreeMap<usize, f64>> {
    let mut by_name: BTreeMap<&'static str, BTreeMap<usize, f64>> = BTreeMap::new();
    for workload in all().into_iter().filter(|w| w.shape == shape) {
        let (_, costs) = measure(|| (workload.run)(workload.n));
        let divisor = if matches!(shape, CostShape::FlatPerEntry | CostShape::QuadraticBuild) {
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
        let (smallest, largest) = span(points);
        let (small, large) = (points[&smallest], points[&largest]);
        // A zero baseline is not a free pass: a ratio cannot express it, so
        // any growth at all is the failure.
        if small == 0.0 {
            if large > 0.0 {
                failures.push(format!(
                    "{name}: {unit} went 0 (n={smallest}) -> {large:.1} (n={largest}); \
                     an operation that did none of this now does some"
                ));
            }
        } else if large > small * MAX_GROWTH {
            failures.push(format!(
                "{name}: {unit} grew {small:.1} (n={smallest}) -> {large:.1} \
                 (n={largest}), {:.1}x - budget is {MAX_GROWTH}x",
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

/// Reads matter most: the VM has no read counter and no read limit, so an
/// O(n) read pattern is invisible until a call dies outright.
#[test]
fn point_operation_cost_does_not_grow_with_collection_size() {
    assert_bounded("reads/call", CostShape::ConstantPerCall, |c| c.rows_read);
    assert_bounded("writes/call", CostShape::ConstantPerCall, |c| {
        c.rows_written
    });
}

/// The ratchet on costs known to be O(n): fails if one gets worse than linear,
/// and fails differently if one stops being linear.
#[test]
fn known_linear_costs_are_still_exactly_linear() {
    let mut failures = Vec::new();

    for (name, points) in &series(CostShape::KnownLinearInN, |c| c.rows_read) {
        let (_, largest) = span(points);
        let floor = largest as f64 / LINEAR_FLOOR_DIVISOR;
        let ceiling = largest as f64 * LINEAR_CEILING_FACTOR;
        let large = points[&largest];

        if large < floor {
            failures.push(format!(
                "{name}: reads/call at n={largest} is {large:.0}, below the {floor:.0} \
                 floor that marks a linear cost — this appears to have been FIXED. That \
                 is good news, and it must be recorded: move it to \
                 CostShape::ConstantPerCall, regenerate \
                 tools/storage-cost/storage-costs.json, and update the wall write-up \
                 on that workload in tools/storage-cost/src/workloads.rs"
            ));
        } else if large > ceiling {
            failures.push(format!(
                "{name}: reads/call at n={largest} is {large:.0}, above the \
                 {ceiling:.0} ceiling — worse than linear, i.e. a regression stacked on \
                 top of a known-bad cost"
            ));
        }
    }

    report(failures, "known-linear costs no longer match their marker");
}

/// The same ratchet for builds known to be `O(n^2)` overall, measured at
/// [`QUADRATIC_SIZES`].
#[test]
fn quadratic_build_costs_are_still_exactly_quadratic() {
    let floor = QUADRATIC_LARGEST as f64 / QUADRATIC_FLOOR_DIVISOR;
    let ceiling = QUADRATIC_LARGEST as f64 * QUADRATIC_CEILING_FACTOR;
    let mut failures = Vec::new();

    for (name, points) in &series(CostShape::QuadraticBuild, |c| c.rows_read) {
        let large = points[&QUADRATIC_LARGEST];

        if large < floor {
            failures.push(format!(
                "{name}: reads/entry at n={QUADRATIC_LARGEST} is {large:.1}, below the \
                 {floor:.1} floor that marks a quadratic build — this appears to have been \
                 FIXED. That is good news, and it must be recorded: move it to \
                 CostShape::FlatPerEntry (or CostShape::KnownLinearInN if only the \
                 per-call cost, not the whole build, was fixed), regenerate \
                 tools/storage-cost/storage-costs.json, and update the read-wall write-up \
                 this workload's doc comment cites."
            ));
        } else if large > ceiling {
            failures.push(format!(
                "{name}: reads/entry at n={QUADRATIC_LARGEST} is {large:.1}, above the \
                 {ceiling:.1} ceiling — worse than quadratic, i.e. a regression stacked on \
                 top of a known-bad cost"
            ));
        }
    }

    report(
        failures,
        "quadratic-build costs no longer match their marker",
    );
}
