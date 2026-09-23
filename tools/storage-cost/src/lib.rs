//! Counts the storage operations a `calimero-storage` workload performs, by
//! installing a `RuntimeEnv` whose callbacks close over an owned map plus
//! counters. Each [`measure`] call gets a fresh map, so runs cannot leak into
//! each other.
//!
//! ```text
//! cargo run -p storage-cost --bin storage-cost --release
//! ```
//!
//! Row counts reproduce exactly and are what [`RowCosts`],
//! `storage-costs.json` and `scripts/check-storage-cost.sh` gate on. Byte
//! counts do not: every entity id is drawn from an unseedable RNG, so index
//! rows serialize to slightly different lengths (~1.5%) run to run. Seeding it
//! would be a production change to `calimero-storage`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::store::Key;
use serde::{Deserialize, Serialize};

pub mod workloads;

/// The deterministic projection of [`Costs`]: what the snapshot gate diffs.
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
}

impl Costs {
    /// The gateable subset; the module docs say why bytes are not in it.
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

    let previous = CURRENT.with(|c| c.borrow_mut().replace(Rc::clone(&backing)));
    let result = with_runtime_env(env, f);
    CURRENT.with(|c| *c.borrow_mut() = previous);

    let costs = backing.borrow().costs;
    (result, costs)
}

thread_local! {
    /// The innermost in-flight [`measure`]'s store, restored on the way out so
    /// nesting behaves.
    static CURRENT: RefCell<Option<Rc<RefCell<Backing>>>> = const { RefCell::new(None) };
}

/// Discard everything counted so far in the enclosing [`measure`] call, which
/// is how a point workload isolates one operation from the build in front of
/// it. A no-op outside `measure`.
pub fn reset_counters() {
    CURRENT.with(|c| {
        if let Some(backing) = c.borrow().as_ref() {
            backing.borrow_mut().costs = Costs::default();
        }
    });
}

#[cfg(test)]
mod tests {
    use calimero_storage::collections::{Root, UnorderedMap};
    use calimero_storage::store::MainStorage;

    use super::*;

    /// If `MainStorage` is not routed through our callbacks, every count is
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

    /// The property the whole snapshot gate rests on. Built with
    /// `new_with_field_name`, not `new`: a random entity id changes how deep
    /// the child trie is descended, so `rows_read` would move on its own.
    #[test]
    fn row_counts_are_deterministic_across_calls() {
        let workload = || {
            let mut map = Root::new(|| {
                UnorderedMap::<String, String, MainStorage>::new_with_field_name("determinism")
            });
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
}
