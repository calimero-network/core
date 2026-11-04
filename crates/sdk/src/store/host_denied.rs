//! [`Store`] in a compute component: its world has no `storage` import, so every access traps.

use super::{RangeCursor, Store};

/// The host's storage as a compute component sees it: none.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostStore;

/// A cursor that cannot exist: `range` traps first.
pub struct HostCursor;

fn denied() -> ! {
    core::arch::wasm32::unreachable()
}

impl RangeCursor for HostCursor {
    fn next(&mut self, _max: u32) -> Vec<(Vec<u8>, Vec<u8>)> {
        denied()
    }
}

impl Store for HostStore {
    type Range = HostCursor;

    fn get(&self, _k: &[u8]) -> Option<Vec<u8>> {
        denied()
    }

    fn put(&mut self, _k: &[u8], _v: &[u8]) {
        denied()
    }

    fn delete(&mut self, _k: &[u8]) {
        denied()
    }

    fn add(&mut self, _k: &[u8], _delta: i64) {
        denied()
    }

    fn range(&self, _start: &[u8], _end: &[u8], _reverse: bool) -> HostCursor {
        denied()
    }
}
