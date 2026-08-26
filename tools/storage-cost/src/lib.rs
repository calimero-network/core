//! Deterministic storage-cost measurement for `calimero-storage`.
//!
//! Installs a `RuntimeEnv` whose read/write/remove callbacks close over an
//! owned map plus counters, so every storage operation a workload performs is
//! counted exactly. Nothing in `crates/` changes: `RuntimeEnv::new` already
//! takes the callbacks as `Rc<dyn Fn>`.
//!
//! Each `measure` call gets a FRESH backing map, so measurements are isolated
//! from each other without needing `env::reset_for_testing` (which is
//! `#[cfg(test)]` and therefore unreachable from this crate).
//!
//! # Determinism, measured 2026-08-26
//!
//! ROW counts are exactly reproducible in-process: three consecutive runs of
//! every registered workload at every size produced identical
//! `rows_read`/`rows_written`/`rows_removed`. No process isolation is needed.
//!
//! BYTE counts are NOT reproducible, and no amount of process isolation would
//! make them so. Every entity gets an `Id::random()`
//! (`crates/storage/src/address.rs:49`), which on the native path is
//! `rand::thread_rng()` (`crates/storage/src/env.rs:1099`) with no seeding hook
//! reachable from outside the crate. Random ids land in different child-trie
//! buckets run to run, so index rows serialize to slightly different lengths —
//! observed drift is ~1.5% at every size.
//!
//! Consequence for the gate: [`RowCosts`] — the row counters only — is what
//! `storage-costs.json` records and what `scripts/check-storage-cost.sh` diffs.
//! Byte counts stay on [`Costs`] for local inspection and for the benches, but
//! gating on them would be a flake generator. Making them gateable needs a
//! seedable RNG hook in `calimero-storage`, which is a production change and
//! deliberately out of scope here.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::store::Key;
use serde::{Deserialize, Serialize};

pub mod workloads;

/// The deterministic projection of [`Costs`]: what the snapshot gate diffs.
///
/// Row counts reproduce exactly; byte counts do not (see the module docs), so
/// only these three cross into `storage-costs.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowCosts {
    /// Host reads issued, whether or not the key existed.
    pub rows_read: u64,
    /// Host writes issued.
    pub rows_written: u64,
    /// Host removes issued.
    pub rows_removed: u64,
}

/// Storage operations performed by one workload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Costs {
    /// Host reads issued, whether or not the key existed.
    pub rows_read: u64,
    /// Host writes issued.
    pub rows_written: u64,
    /// Host removes issued.
    pub rows_removed: u64,
    /// Bytes handed to the write callback.
    pub bytes_written: u64,
    /// Bytes returned by the read callback (misses contribute nothing).
    pub bytes_read: u64,
}

impl Costs {
    /// The gateable subset. See the module docs for why bytes are excluded.
    #[must_use]
    pub const fn rows(&self) -> RowCosts {
        RowCosts {
            rows_read: self.rows_read,
            rows_written: self.rows_written,
            rows_removed: self.rows_removed,
        }
    }
}

#[derive(Default)]
struct Backing {
    map: BTreeMap<[u8; 32], Vec<u8>>,
    costs: Costs,
}

/// Run `f` against a fresh counting store, returning its result and the
/// storage operations it performed.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Costs) {
    let backing = Rc::new(RefCell::new(Backing::default()));

    let read = {
        let b = Rc::clone(&backing);
        Rc::new(move |key: &Key| {
            let mut b = b.borrow_mut();
            let value = b.map.get(&key.to_bytes()).cloned();
            b.costs.rows_read += 1;
            if let Some(bytes) = value.as_ref() {
                b.costs.bytes_read += bytes.len() as u64;
            }
            value
        })
    };

    let write = {
        let b = Rc::clone(&backing);
        Rc::new(move |key: Key, value: &[u8]| {
            let mut b = b.borrow_mut();
            b.costs.rows_written += 1;
            b.costs.bytes_written += value.len() as u64;
            let _ignored = b.map.insert(key.to_bytes(), value.to_vec());
            true
        })
    };

    let remove = {
        let b = Rc::clone(&backing);
        Rc::new(move |key: &Key| {
            let mut b = b.borrow_mut();
            b.costs.rows_removed += 1;
            b.map.remove(&key.to_bytes()).is_some()
        })
    };

    let env = RuntimeEnv::new(read, write, remove, [1; 32], [2; 32], [3; 32]);
    let result = with_runtime_env(env, f);
    let costs = backing.borrow().costs;
    (result, costs)
}

#[cfg(test)]
mod tests {
    use calimero_storage::collections::{Root, UnorderedMap};
    use calimero_storage::store::MainStorage;

    use super::*;

    /// The harness must observe writes at all — if the `RuntimeEnv` is not
    /// actually routing `MainStorage` through our callbacks, every count is
    /// silently zero and every downstream gate is vacuous.
    #[test]
    fn measure_observes_writes() {
        let (_, costs) = measure(|| {
            let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
            map.insert("k".to_owned(), "v".to_owned())
                .expect("insert should succeed");
        });

        assert!(
            costs.rows_written > 0,
            "harness observed no writes — RuntimeEnv is not routing MainStorage; got {costs:?}"
        );
        assert!(
            costs.bytes_written > 0,
            "harness observed no bytes written; got {costs:?}"
        );
    }

    /// Two identical measurements must produce identical ROW counts. This is
    /// the property the entire snapshot gate rests on: if per-process
    /// thread-local state leaks between measurements, the snapshot is unstable
    /// and the gate flakes.
    #[test]
    fn row_counts_are_deterministic_across_calls() {
        let workload = || {
            let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
            for i in 0..16 {
                map.insert(format!("k{i}"), "v".to_owned())
                    .expect("insert should succeed");
            }
        };

        let (_, first) = measure(workload);
        let (_, second) = measure(workload);

        assert_eq!(
            first.rows(),
            second.rows(),
            "identical workloads produced different row counts — state is leaking \
             between measurements, and the snapshot gate cannot be trusted"
        );
    }

    /// Pins the reason bytes are not gated, so a future contributor who wants
    /// to add them to the snapshot finds out here rather than from a flaky CI
    /// run. If this ever FAILS, entity ids have become deterministic and
    /// `bytes_written`/`bytes_read` can be promoted into `RowCosts`.
    #[test]
    fn byte_counts_are_not_reproducible() {
        let workload = || {
            let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
            for i in 0..256 {
                map.insert(format!("k{i}"), "v".to_owned())
                    .expect("insert should succeed");
            }
        };

        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..8 {
            let (_, costs) = measure(workload);
            let _ignored = seen.insert(costs.bytes_written);
        }

        assert!(
            seen.len() > 1,
            "byte counts reproduced exactly across 8 runs ({seen:?}) — entity ids may \
             have become deterministic. If so, promote bytes into RowCosts and delete \
             this test; see the module docs."
        );
    }
}
