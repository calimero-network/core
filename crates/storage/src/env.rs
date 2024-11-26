//! Environment bindings for the storage crate.

#[cfg(target_arch = "wasm32")]
use calimero_vm as imp;
#[cfg(not(target_arch = "wasm32"))]
use mocked as imp;

use crate::store::Key;

/// Commits the root hash to the runtime.
///
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
    imp::commit(root_hash, artifact);
}

/// Reads data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to read data from.
///
#[must_use]
pub fn storage_read(key: Key) -> Option<Vec<u8>> {
    imp::storage_read(key)
}

/// Removes data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to remove.
///
#[must_use]
pub fn storage_remove(key: Key) -> bool {
    imp::storage_remove(key)
}

/// Writes data to persistent storage.
///
/// # Parameters
///
/// * `key`   - The key to write data to.
/// * `value` - The data to write.
///
#[must_use]
pub fn storage_write(key: Key, value: &[u8]) -> bool {
    imp::storage_write(key, value)
}

/// Fill the buffer with random bytes.
///
/// # Parameters
///
/// * `buf` - The buffer to fill with random bytes.
///
pub fn random_bytes(buf: &mut [u8]) {
    imp::random_bytes(buf);
}

/// Get the current time.
#[must_use]
pub fn time_now() -> u64 {
    imp::time_now()
}

/// Return the context id.
#[must_use]
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn context_id() -> [u8; 32] {
    imp::context_id()
}

#[cfg(target_arch = "wasm32")]
mod calimero_vm {
    use calimero_sdk::env;

    use crate::store::Key;

    /// Commits the root hash to the runtime.
    pub(super) fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
        env::commit(root_hash, artifact);
    }

    /// Reads data from persistent storage.
    pub(super) fn storage_read(key: Key) -> Option<Vec<u8>> {
        env::storage_read(&key.to_bytes())
    }

    /// Removes data from persistent storage.
    pub(super) fn storage_remove(key: Key) -> bool {
        env::storage_remove(&key.to_bytes())
    }

    /// Writes data to persistent storage.
    pub(super) fn storage_write(key: Key, value: &[u8]) -> bool {
        env::storage_write(&key.to_bytes(), value)
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        env::random_bytes(buf)
    }

    /// Return the context id.
    pub(super) fn context_id() -> [u8; 32] {
        env::context_id()
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp.
    ///
    pub(super) fn time_now() -> u64 {
        env::time_now()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod mocked {
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rand::RngCore;

    use crate::store::{Key, MockedStorage, StorageAdaptor};

    thread_local! {
        static ROOT_HASH: RefCell<Option<[u8; 32]>> = const { RefCell::new(None) };
    }

    /// The default storage system.
    type DefaultStore = MockedStorage<{ usize::MAX }>;

    /// Commits the root hash to the runtime.
    pub(super) fn commit(root_hash: &[u8; 32], _artifact: &[u8]) {
        ROOT_HASH.with(|rh| {
            if rh.borrow_mut().replace(*root_hash).is_some() {
                Option::expect(None, "State previously committed")
            }
        });
    }

    /// Reads data from persistent storage.
    pub(super) fn storage_read(key: Key) -> Option<Vec<u8>> {
        DefaultStore::storage_read(key)
    }

    /// Removes data from persistent storage.
    pub(super) fn storage_remove(key: Key) -> bool {
        DefaultStore::storage_remove(key)
    }

    /// Writes data to persistent storage.
    pub(super) fn storage_write(key: Key, value: &[u8]) -> bool {
        DefaultStore::storage_write(key, value)
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        rand::thread_rng().fill_bytes(buf);
    }

    /// Return the context id.
    pub(super) const fn context_id() -> [u8; 32] {
        [236; 32]
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp.
    ///
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(clippy::expect_used, reason = "Effectively infallible here")]
    pub(super) fn time_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64
    }
}
