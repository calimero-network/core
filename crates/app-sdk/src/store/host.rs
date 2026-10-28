//! [`Store`] over the `calimero:app/storage` host interface.

use super::{RangeCursor, Store};
use crate::bindings::calimero::app::storage;

/// The host's storage; a zero-sized handle.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostStore;

/// A cursor over the host's `cursor` resource.
pub struct HostCursor(storage::Cursor);

impl RangeCursor for HostCursor {
    fn next(&mut self, max: u32) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.0.next(max)
    }
}

impl Store for HostStore {
    type Range = HostCursor;

    fn get(&self, k: &[u8]) -> Option<Vec<u8>> {
        storage::get(k)
    }

    fn put(&mut self, k: &[u8], v: &[u8]) {
        storage::put(k, v);
    }

    fn delete(&mut self, k: &[u8]) {
        storage::delete(k);
    }

    fn add(&mut self, k: &[u8], delta: i64) {
        storage::add(k, delta);
    }

    fn range(&self, start: &[u8], end: &[u8], reverse: bool) -> HostCursor {
        HostCursor(storage::range(start, end, reverse))
    }
}
