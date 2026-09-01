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

use storage_cost::workloads::{all, CostShape, QUADRATIC_SIZES, SIZES};
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

/// A `QuadraticBuild` reads/entry (i.e. average per-call cost over the whole
/// build — see `series`'s `FlatPerEntry` divisor, which this shape also
/// uses) must be at least `n / QUADRATIC_FLOOR_DIVISOR` at the largest
/// measured size, for the same reason `LINEAR_FLOOR_DIVISOR` exists: a value
/// that falls below this means the per-call cost has stopped scaling with
/// `n` at all, i.e. the build is no longer quadratic.
///
/// Calibrated, not guessed: `rga_insert_per_char` measures `2047.0`
/// reads/entry at `n=2_000` (see its doc comment). Swapping the per-char
/// loop for the flat, single-linearisation `insert_str` (the actual
/// candidate fix — see `rga_insert`) measures `48.0` reads/entry at the same
/// `n`, over 40x lower. `5.0` puts the floor at `400.0` — comfortably below
/// the quadratic value and comfortably above the flat one, so this
/// direction was verified to fire (and only fire) on the real "it got
/// fixed" case, not tuned in the abstract.
const QUADRATIC_FLOOR_DIVISOR: f64 = 5.0;

/// …and at most `n * QUADRATIC_CEILING_FACTOR`. Past that, the per-call cost
/// is growing faster than the document itself — worse than quadratic.
const QUADRATIC_CEILING_FACTOR: f64 = 4.0;

const QUADRATIC_LARGEST: usize = QUADRATIC_SIZES[QUADRATIC_SIZES.len() - 1];

/// Measured cost of every workload of `shape`, as `name -> n -> cost`.
///
/// `FlatPerEntry` and `QuadraticBuild` costs are divided by `n`, because both
/// are workloads that deliberately do `n` operations and what is under test
/// is the AVERAGE cost of one — flat for the former, growing with `n` for
/// the latter. Point workloads (`ConstantPerCall`, `KnownLinearInN`) are not
/// divided: they perform exactly one operation, and its absolute cost is the
/// thing under test.
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
        let (small, large) = (points[&SMALLEST], points[&LARGEST]);
        // A zero baseline is not a free pass. `small == 0.0` means the workload
        // does none of this metric at the smallest size — `unordered_map_get`
        // and `unordered_map_len` write nothing, and should keep writing
        // nothing. A ratio cannot express that (everything is infinity), so
        // any growth at all is the failure, and skipping the check instead
        // would leave the gate blind in exactly the case it exists for: an
        // operation that starts doing something it never used to.
        if small == 0.0 {
            if large > 0.0 {
                failures.push(format!(
                    "{name}: {unit} went 0 (n={SMALLEST}) -> {large:.1} (n={LARGEST}); \
                     an operation that did none of this now does some"
                ));
            }
        } else if large > small * MAX_GROWTH {
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

/// The ratchet on BUILDS we know are `O(n^2)` overall — a per-call cost that
/// is itself `O(n)`, paid `n` times — and have chosen not to fix yet.
///
/// Same both-directions shape as `known_linear_costs_are_still_exactly_
/// linear`: fails if the curve gets worse than quadratic, and fails —
/// differently, and on purpose — if the per-call cost stops growing with
/// `n` (i.e. someone fixed the underlying `O(n)` re-linearisation and this
/// marker was not moved to record it).
///
/// Measured at [`QUADRATIC_SIZES`], not [`SIZES`] — see that constant's doc
/// comment on why a `QuadraticBuild` workload needs its own, smaller sizes.
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
