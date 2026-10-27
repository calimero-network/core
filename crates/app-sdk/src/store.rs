//! The ordered key-value store the SDK runs on: [`HostStore`] in the guest, [`MemStore`] in tests.

use crate::error::StorageError;

mod mem;

#[cfg(target_arch = "wasm32")]
mod host;

pub use mem::MemStore;

#[cfg(target_arch = "wasm32")]
pub use host::HostStore;

/// The store tables and collections use when none is given.
#[cfg(target_arch = "wasm32")]
pub type DefaultStore = HostStore;
/// The store tables and collections use when none is given.
#[cfg(not(target_arch = "wasm32"))]
pub type DefaultStore = MemStore;

/// The longest key the host accepts (D50).
pub const MAX_KEY_BYTES: usize = 1024;
/// The most entries the host returns for one cursor read.
pub const MAX_RANGE_BATCH: u32 = 1000;

/// A forward or reverse read of a key range, a batch at a time.
pub trait RangeCursor {
    /// Up to `max` (at most [`MAX_RANGE_BATCH`]) further entries; empty at the end.
    fn next(&mut self, max: u32) -> Vec<(Vec<u8>, Vec<u8>)>;
}

/// Ordered key-value storage, one-to-one with the `calimero:app/storage` interface.
pub trait Store {
    /// The cursor type [`Store::range`] opens.
    type Range: RangeCursor;

    /// The value at `k`.
    fn get(&self, k: &[u8]) -> Option<Vec<u8>>;
    /// Set `k` to `v`.
    fn put(&mut self, k: &[u8], v: &[u8]);
    /// Remove `k`.
    fn delete(&mut self, k: &[u8]);
    /// Add `delta` to the CBOR integer at `k` (absent is 0) without reading it (D47).
    fn add(&mut self, k: &[u8], delta: i64);
    /// Open a cursor over `[start, end)`, newest-first when `reverse`.
    fn range(&self, start: &[u8], end: &[u8], reverse: bool) -> Self::Range;
}

/// Refuse a key the host would refuse, with a clear error before the host call (D50).
pub fn check_key(key: &[u8]) -> Result<(), StorageError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(StorageError::KeyTooLong {
            len: key.len(),
            max: MAX_KEY_BYTES,
        });
    }
    Ok(())
}
