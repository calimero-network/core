//! An in-memory [`Store`] with the host's semantics, for tests on the host target.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::rc::Rc;

use super::{RangeCursor, Store, MAX_RANGE_BATCH};
use crate::encoding::value;

#[derive(Default)]
struct Inner {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    pulled: usize,
    writes: usize,
}

/// A shared in-memory store: clones see the same data.
#[derive(Clone)]
pub struct MemStore {
    inner: Rc<RefCell<Inner>>,
}

thread_local! {
    static AMBIENT: MemStore = MemStore::new();
}

impl MemStore {
    /// A new, empty store.
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner::default())),
        }
    }

    /// How many entries cursors of this store have returned so far.
    pub fn pulled(&self) -> usize {
        self.inner.borrow().pulled
    }

    /// Reset the pulled-entries counter.
    pub fn reset_pulled(&self) {
        self.inner.borrow_mut().pulled = 0;
    }

    /// How many writes (`put`, `delete`, `add`) this store has seen.
    pub fn writes(&self) -> usize {
        self.inner.borrow().writes
    }

    /// How many entries the store holds.
    pub fn len(&self) -> usize {
        self.inner.borrow().map.len()
    }

    /// Whether the store holds no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.borrow().map.is_empty()
    }
}

impl Default for MemStore {
    /// A handle to this thread's shared store, so `Table::new` works in tests without wiring.
    fn default() -> Self {
        AMBIENT.with(Self::clone)
    }
}

impl Store for MemStore {
    type Range = MemCursor;

    fn get(&self, k: &[u8]) -> Option<Vec<u8>> {
        self.inner.borrow().map.get(k).cloned()
    }

    fn put(&mut self, k: &[u8], v: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        inner.writes += 1;
        inner.map.insert(k.to_vec(), v.to_vec());
    }

    fn delete(&mut self, k: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        inner.writes += 1;
        inner.map.remove(k);
    }

    fn add(&mut self, k: &[u8], delta: i64) {
        let mut inner = self.inner.borrow_mut();
        inner.writes += 1;
        let current = match inner.map.get(k) {
            None => 0,
            // SAFETY: the host traps `HostDenied` on a non-integer; a test hitting this is a bug.
            Some(bytes) => value::int_from_slice(bytes).expect("add to a non-integer value"),
        };
        // SAFETY: the sum of an i128 CBOR integer and an i64 stays in range unless the host
        // would trap too.
        let sum = value::int_to_vec(current + i128::from(delta)).expect("sum out of range");
        inner.map.insert(k.to_vec(), sum);
    }

    fn range(&self, start: &[u8], end: &[u8], reverse: bool) -> MemCursor {
        MemCursor {
            inner: Rc::clone(&self.inner),
            start: start.to_vec(),
            end: end.to_vec(),
            reverse,
            last: None,
        }
    }
}

/// A lazy cursor: every batch re-ranges from the last returned key, like the host's.
pub struct MemCursor {
    inner: Rc<RefCell<Inner>>,
    start: Vec<u8>,
    end: Vec<u8>,
    reverse: bool,
    last: Option<Vec<u8>>,
}

impl RangeCursor for MemCursor {
    fn next(&mut self, max: u32) -> Vec<(Vec<u8>, Vec<u8>)> {
        let max = max.min(MAX_RANGE_BATCH) as usize;
        let mut inner = self.inner.borrow_mut();
        if max == 0 || self.start >= self.end {
            return Vec::new();
        }
        let batch: Vec<(Vec<u8>, Vec<u8>)> = if self.reverse {
            let upper = self.last.as_ref().unwrap_or(&self.end);
            if *upper <= self.start {
                return Vec::new();
            }
            inner
                .map
                .range::<[u8], _>((
                    Bound::Included(self.start.as_slice()),
                    Bound::Excluded(upper.as_slice()),
                ))
                .rev()
                .take(max)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        } else {
            let lower = match &self.last {
                Some(last) => Bound::Excluded(last.as_slice()),
                None => Bound::Included(self.start.as_slice()),
            };
            inner
                .map
                .range::<[u8], _>((lower, Bound::Excluded(self.end.as_slice())))
                .take(max)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        inner.pulled += batch.len();
        if let Some((k, _)) = batch.last() {
            self.last = Some(k.clone());
        }
        batch
    }
}
