//! The single registry of measured workloads.
//!
//! The binary, the flat-curve tests and the criterion benches all iterate
//! `all()`. Defining a workload anywhere else would let the gate and the
//! benchmarks measure different things while claiming to measure one.
//!
//! # Two kinds of workload
//!
//! A *build* workload does `n` operations and is measured whole: its total cost
//! is expected to grow with `n`, and what must stay flat is cost **per entry**.
//!
//! A *point* workload builds `n` entries, calls [`crate::reset_counters`], and
//! then performs exactly one operation. What it reports is the cost of that one
//! operation with `n` entries already in the collection — which is the number
//! that decides whether a collection stays readable as it grows.
//!
//! [`CostShape`] says which is which, and, for point workloads, whether the
//! curve is required to be flat or is a known-linear cost being held under
//! observation.

use calimero_storage::collections::{Root, UnorderedMap, UnorderedSet, Vector};
use calimero_storage::store::MainStorage;

use crate::reset_counters;

/// Collection sizes every workload is measured at. The gate compares costs at
/// each size; the shape tests compare the first against the last.
pub const SIZES: [usize; 4] = [10, 100, 1_000, 10_000];

/// What the cost curve of a workload is required to look like.
///
/// This is an assertion, not a description. Every variant is checked by
/// `tests/flat_curve.rs`, including [`Self::KnownLinearInN`] — a workload that
/// stops being linear fails just as loudly as one that starts being linear,
/// because "we fixed it and nobody updated the marker" and "we broke something
/// else" must not be told apart by guesswork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostShape {
    /// A build of `n` entries. Cost **per entry** must not grow with `n`.
    FlatPerEntry,
    /// One operation against a collection of `n` entries. Its **total** cost
    /// must not grow with `n`.
    ConstantPerCall,
    /// One operation whose total cost is KNOWN to grow linearly with `n`.
    ///
    /// This is not a licence — it is a ratchet. See `ordered_read` below.
    KnownLinearInN,
}

/// One measurable unit of work at one collection size.
pub struct Workload {
    /// Stable identifier. Appears in the snapshot, so renaming one is a
    /// snapshot change a reviewer will see.
    pub name: &'static str,
    /// Collection size this instance exercises.
    pub n: usize,
    /// The curve this workload's cost is asserted to follow.
    pub shape: CostShape,
    /// How far a measured row count may sit from the committed snapshot before
    /// the gate fails, as a percentage.
    ///
    /// Zero for almost everything: row counts reproduce exactly. It is nonzero
    /// only where the operation walks the WHOLE child trie, because the trie's
    /// node count depends on how random entity ids happened to distribute, so
    /// the number of node reads varies run to run.
    ///
    /// This is a measured property, not a guess — `tests/reproducible.rs`
    /// re-derives the spread of every workload and fails if a declared
    /// tolerance is either too tight (flaky gate) or gratuitously loose
    /// (blind gate).
    pub tolerance_pct: u32,
    /// Builds a collection of `n` entries and performs the measured operation.
    pub run: fn(usize),
}

/// Insert `n` entries into an `UnorderedMap`, measuring the whole build.
fn unordered_map_insert(n: usize) {
    build_map(n);
}

/// Push `n` entries onto a `Vector`, measuring the whole build.
fn vector_push(n: usize) {
    build_vector(n);
}

/// Insert `n` entries into an `UnorderedSet`, measuring the whole build.
fn unordered_set_insert(n: usize) {
    let mut set = Root::new(UnorderedSet::<String, MainStorage>::new);
    for i in 0..n {
        let _ignored = set
            .insert(format!("value{i}"))
            .expect("insert should succeed");
    }
}

/// Cost of ONE `len()` against `n` entries. `len()` reading the whole
/// collection to count it was core#3602 finding 2.
fn unordered_map_len(n: usize) {
    let map = build_map(n);
    reset_counters();
    let _ignored = map.len().expect("len should succeed");
}

/// Cost of ONE keyed `get()` against `n` entries.
fn unordered_map_get(n: usize) {
    let map = build_map(n);
    reset_counters();
    let _ignored = map.get("key0").expect("get should succeed");
}

/// Cost of ONE positional read — `Vector::get(i)` — against `n` entries.
///
/// # Why this is `KnownLinearInN`, and what it is standing in for
///
/// This is the in-repo fixture for the read wall documented in
/// `docs/superpowers/2026-08-26-chat-read-wall.md`: mero-chat's `get_messages`
/// exhausts a 1e9 gas budget at ~32,000 messages, and 30,000 already spends
/// 99.83% of it. The cause is not the app. It is this call:
///
/// ```text
/// Vector::get(i) -> Collection::nth(i) -> children_cache()
///                -> Index::get_children_of(parent)
///                -> ChildTrie::children()   // collect ALL, then sort
/// ```
///
/// One O(n) id walk per positional read, because order lives in a comparator
/// applied after a full enumeration rather than in the structure. The child
/// trie bounded *write* cost; it never touched ordered *read* cost.
///
/// Fixing that is a real project (see the ordering design doc) and is
/// deliberately not attempted here. What this workload does is make the cost
/// **gated** instead of merely known: the snapshot pins the constant, and
/// `tests/flat_curve.rs` pins the slope. Nobody can make it quietly worse, and
/// nobody can fix it without the marker below going red and forcing this
/// comment to be rewritten.
///
/// The middle index is read, not the first, so a hypothetical fast path for
/// index 0 could not make the measurement lie.
fn vector_get_nth(n: usize) {
    let vector = build_vector(n);
    reset_counters();
    let _ignored = vector.get(n / 2).expect("get should succeed");
}

fn build_map(n: usize) -> Root<UnorderedMap<String, String, MainStorage>> {
    let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
    for i in 0..n {
        map.insert(format!("key{i}"), "value".to_owned())
            .expect("insert should succeed");
    }
    map
}

fn build_vector(n: usize) -> Root<Vector<String, MainStorage>> {
    let mut vector = Root::new(Vector::<String, MainStorage>::new);
    for i in 0..n {
        vector
            .push(format!("value{i}"))
            .expect("push should succeed");
    }
    vector
}

/// Every workload at every size.
///
/// `SortedMap` and `SortedSet` are deliberately absent.
///
/// Their cost depends on `StorageAdaptor::index_supported()`
/// (`crates/storage/src/store.rs`). Without `RuntimeEnv::with_index` installed,
/// native ordered-index ops fall through to the process thread-local mock
/// (`crates/storage/src/env.rs`), so measuring them here would publish numbers
/// for the in-memory-sort fallback while appearing to describe the indexed path
/// a real node takes. Adding them means wiring all eight `IndexCallbacks`
/// through the counting store first — a separate piece of work, not a workload
/// entry.
pub fn all() -> Vec<Workload> {
    use CostShape::{ConstantPerCall, FlatPerEntry, KnownLinearInN};

    const REGISTRY: [(&str, CostShape, u32, fn(usize)); 6] = [
        (
            "unordered_map_insert",
            FlatPerEntry,
            0,
            unordered_map_insert,
        ),
        (
            "unordered_set_insert",
            FlatPerEntry,
            0,
            unordered_set_insert,
        ),
        ("vector_push", FlatPerEntry, 0, vector_push),
        ("unordered_map_len", ConstantPerCall, 0, unordered_map_len),
        ("unordered_map_get", ConstantPerCall, 0, unordered_map_get),
        // Walks the whole trie, so its node count follows the random id
        // distribution. Measured spread over seven runs: 10.5% at n=10,
        // under 3% at every larger size.
        ("vector_get_nth", KnownLinearInN, 25, vector_get_nth),
    ];

    let mut out = Vec::with_capacity(REGISTRY.len() * SIZES.len());
    for n in SIZES {
        for (name, shape, tolerance_pct, run) in REGISTRY {
            out.push(Workload {
                name,
                n,
                shape,
                tolerance_pct,
                run,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure;

    #[test]
    fn every_workload_is_measurable_and_touches_storage() {
        for workload in all() {
            let (_, costs) = measure(|| (workload.run)(workload.n));
            let touched = costs.rows_read + costs.rows_written + costs.rows_removed;
            assert!(
                touched > 0,
                "workload {} at n={} performed no storage operations: {costs:?} — a \
                 workload that measures nothing gates nothing",
                workload.name,
                workload.n
            );
        }
    }

    /// A point workload that forgot `reset_counters()` would silently report
    /// its build cost and look linear no matter what the operation does.
    #[test]
    fn point_workloads_cost_far_less_than_the_build_they_follow() {
        let n = 1_000;
        let (_, build) = measure(|| unordered_map_insert(n));
        let (_, point) = measure(|| unordered_map_get(n));

        assert!(
            point.rows_read * 10 < build.rows_read,
            "unordered_map_get at n={n} read {} rows against a build of {} — it is \
             reporting the build, so reset_counters() is not taking effect",
            point.rows_read,
            build.rows_read
        );
    }

    #[test]
    fn workload_names_and_sizes_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for workload in all() {
            assert!(
                seen.insert((workload.name, workload.n)),
                "duplicate workload {} at n={}",
                workload.name,
                workload.n
            );
        }
    }
}
