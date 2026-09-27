//! Counts the storage operations a `calimero-storage` workload performs, by
//! installing a `RuntimeEnv` whose callbacks close over an owned map plus
//! counters. Each [`measure`] call gets a fresh map, so runs cannot leak into
//! each other.
//!
//! ```text
//! cargo run -p storage-cost --bin storage-cost --release
//! ```
//!
//! The node-local ordered index (`SortedMap`, `IndexedMap`) is routed through
//! the same backing via [`IndexCallbacks`] and counted separately, as
//! `index_rows_*`: it is a different column on a node, never synced and never
//! hashed, so folding it into the state counts would hide what a change moved.
//!
//! Row counts reproduce exactly and are what [`RowCosts`],
//! `storage-costs.json` and `scripts/check-storage-cost.sh` gate on. Byte
//! counts do not: every entity id is drawn from an unseedable RNG, so index
//! rows serialize to slightly different lengths (~1.5%) run to run. Seeding it
//! would be a production change to `calimero-storage`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::env::{with_runtime_env, IndexCallbacks, RuntimeEnv};
use calimero_storage::store::Key;
use serde::{Deserialize, Serialize};

pub mod workloads;

/// The deterministic projection of [`Costs`]: what the snapshot gate diffs.
///
/// The index counts are omitted from the JSON when zero, so a workload that
/// touches no index keeps its snapshot entry byte for byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowCosts {
    /// Host reads issued, whether or not the key existed.
    pub rows_read: u64,
    /// Host writes issued.
    pub rows_written: u64,
    /// Host removes issued.
    pub rows_removed: u64,
    /// Ordered-index rows examined: each row a scan or seek returns, a call
    /// that returns none counting as one, plus each validity-marker read.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index_rows_read: u64,
    /// Ordered-index rows and validity markers written.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index_rows_written: u64,
    /// Ordered-index rows and validity markers removed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index_rows_removed: u64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// See [`RowCosts::index_rows_read`].
    pub index_rows_read: u64,
    /// See [`RowCosts::index_rows_written`].
    pub index_rows_written: u64,
    /// See [`RowCosts::index_rows_removed`].
    pub index_rows_removed: u64,
}

impl Costs {
    /// The gateable subset; the module docs say why bytes are not in it.
    #[must_use]
    pub const fn rows(&self) -> RowCosts {
        RowCosts {
            rows_read: self.rows_read,
            rows_written: self.rows_written,
            rows_removed: self.rows_removed,
            index_rows_read: self.index_rows_read,
            index_rows_written: self.index_rows_written,
            index_rows_removed: self.index_rows_removed,
        }
    }
}

#[derive(Default)]
struct Backing {
    map: BTreeMap<[u8; 32], Vec<u8>>,
    /// The ordered index, keyed `collection ‖ order_key` like the node's column.
    index: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Validity markers, keyed by the id they guard.
    index_meta: BTreeMap<Vec<u8>, Vec<u8>>,
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

    let env = RuntimeEnv::new(read, write, remove, [1; 32], [2; 32], [3; 32])
        .with_index(counting_index(&backing));

    let previous = CURRENT.with(|c| c.borrow_mut().replace(Rc::clone(&backing)));
    let result = with_runtime_env(env, f);
    CURRENT.with(|c| *c.borrow_mut() = previous);

    let costs = backing.borrow().costs;
    (result, costs)
}

/// Index callbacks over `backing`'s own index maps, counting as they go.
fn counting_index(backing: &Rc<RefCell<Backing>>) -> IndexCallbacks {
    let b = Rc::clone(backing);
    let set = Rc::new(move |key: &[u8], value: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_written += 1;
        let _ignored = b.index.insert(key.to_vec(), value.to_vec());
        true
    });
    let b = Rc::clone(backing);
    let remove = Rc::new(move |key: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_removed += 1;
        let _ignored = b.index.remove(key);
        true
    });
    let b = Rc::clone(backing);
    let remove_prefix = Rc::new(move |prefix: &[u8]| {
        let mut b = b.borrow_mut();
        let before = b.index.len();
        b.index.retain(|k, _| !k.starts_with(prefix));
        let removed = (before - b.index.len()) as u64;
        b.costs.index_rows_removed += removed.max(1);
        true
    });
    let b = Rc::clone(backing);
    let scan = Rc::new(
        move |lo: &[u8], hi: &[u8], offset: usize, limit: Option<usize>| {
            let mut b = b.borrow_mut();
            let walked = b.index.range(lo.to_vec()..hi.to_vec()).skip(offset);
            let rows: Vec<(Vec<u8>, Vec<u8>)> = match limit {
                Some(n) => walked
                    .take(n)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                None => walked.map(|(k, v)| (k.clone(), v.clone())).collect(),
            };
            // A skipped row is walked too, so it is examined.
            b.costs.index_rows_read += (offset + rows.len()).max(1) as u64;
            rows
        },
    );
    let b = Rc::clone(backing);
    let last = Rc::new(move |lo: &[u8], hi: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_read += 1;
        b.index
            .range(lo.to_vec()..hi.to_vec())
            .next_back()
            .map(|(k, v)| (k.clone(), v.clone()))
    });
    let b = Rc::clone(backing);
    let meta_set = Rc::new(move |key: &[u8], value: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_written += 1;
        let _ignored = b.index_meta.insert(key.to_vec(), value.to_vec());
        true
    });
    let b = Rc::clone(backing);
    let meta_get = Rc::new(move |key: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_read += 1;
        b.index_meta.get(key).cloned()
    });
    let b = Rc::clone(backing);
    let meta_clear = Rc::new(move |key: &[u8]| {
        let mut b = b.borrow_mut();
        b.costs.index_rows_removed += 1;
        let _ignored = b.index_meta.remove(key);
        true
    });
    IndexCallbacks {
        set,
        remove,
        remove_prefix,
        scan,
        last,
        meta_set,
        meta_get,
        meta_clear,
    }
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
