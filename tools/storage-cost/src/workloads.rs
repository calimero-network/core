//! The single registry of measured workloads.
//!
//! The binary, the flat-curve tests and the criterion benches all iterate
//! `all()`. Defining a workload anywhere else would let the gate and the
//! benchmarks measure different things while claiming to measure one.

use calimero_storage::collections::{Root, UnorderedMap};
use calimero_storage::store::MainStorage;

/// Collection sizes every workload is measured at. The gate compares costs at
/// each size; the flat-curve test compares the first against the last.
pub const SIZES: [usize; 4] = [10, 100, 1_000, 10_000];

/// One measurable unit of work at one collection size.
pub struct Workload {
    /// Stable identifier. Appears in the snapshot, so renaming one is a
    /// snapshot change a reviewer will see.
    pub name: &'static str,
    /// Collection size this instance exercises.
    pub n: usize,
    /// Builds a collection of `n` entries and performs the measured operation.
    pub run: fn(usize),
}

/// Insert `n` entries into an `UnorderedMap`, measuring the whole build.
fn unordered_map_insert(n: usize) {
    let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
    for i in 0..n {
        map.insert(format!("key{i}"), "value".to_owned())
            .expect("insert should succeed");
    }
}

/// Build `n` entries, then call `len()` once. Subtracting the matching
/// `unordered_map_insert` measurement isolates what a single `len()` costs —
/// which is how core#3602 finding 2 was characterised.
fn unordered_map_len(n: usize) {
    let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
    for i in 0..n {
        map.insert(format!("key{i}"), "value".to_owned())
            .expect("insert should succeed");
    }
    let _ignored = map.len().expect("len should succeed");
}

/// Every workload at every size.
pub fn all() -> Vec<Workload> {
    let mut out = Vec::new();
    for n in SIZES {
        out.push(Workload {
            name: "unordered_map_insert",
            n,
            run: unordered_map_insert,
        });
        out.push(Workload {
            name: "unordered_map_len",
            n,
            run: unordered_map_len,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure;

    #[test]
    fn every_workload_is_measurable_and_writes_something() {
        for workload in all() {
            let (_, costs) = measure(|| (workload.run)(workload.n));
            assert!(
                costs.rows_written > 0,
                "workload {} at n={} wrote nothing: {costs:?}",
                workload.name,
                workload.n
            );
        }
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
