//! [`Store`] over the `calimero:app/storage` host interface.

use super::{RangeCursor, Store};
use crate::bindings::calimero::app::storage;
use crate::error::{RunLimit, StorageError};

/// The host's answer to a write past one of the run's limits (WIT `limit-exceeded`).
fn run_limit(e: storage::LimitExceeded) -> StorageError {
    StorageError::RunLimit {
        limit: match e.limit {
            storage::RunLimit::Writes => RunLimit::Writes,
            storage::RunLimit::Bytes => RunLimit::Bytes,
        },
        max: e.max,
        attempted: e.attempted,
    }
}

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

    fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), StorageError> {
        storage::put_checked(k, v).map_err(run_limit)
    }

    fn delete(&mut self, k: &[u8]) -> Result<(), StorageError> {
        storage::delete_checked(k).map_err(run_limit)
    }

    fn add(&mut self, k: &[u8], delta: i64) -> Result<(), StorageError> {
        storage::add_checked(k, delta).map_err(run_limit)
    }

    fn range(&self, start: &[u8], end: &[u8], reverse: bool) -> HostCursor {
        HostCursor(storage::range(start, end, reverse))
    }
}
