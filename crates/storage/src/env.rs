//! Environment bindings for the storage crate.

#[cfg(target_arch = "wasm32")]
use calimero_vm as imp;
#[cfg(not(target_arch = "wasm32"))]
use mocked as imp;

use crate::logical_clock::HybridTimestamp;
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

/// Returns the current context ID.
///
/// In WASM, this calls the host function. In tests, returns a fixed value.
#[must_use]
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn context_id() -> [u8; 32] {
    imp::context_id()
}

/// Returns the current executor ID (the public key of the transaction signer).
///
/// In WASM, this calls the host function. In tests, returns a fixed value.
#[must_use]
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn executor_id() -> [u8; 32] {
    imp::executor_id()
}

/// Get hybrid timestamp (auto-increments logical clock).
#[must_use]
pub fn hlc_timestamp() -> HybridTimestamp {
    imp::hlc_timestamp()
}

/// Update HLC with remote timestamp (preserves causality).
///
/// When syncing deltas from remote nodes, call this with each delta's HLC timestamp
/// to ensure the local clock observes remote operations and maintains causal ordering.
///
/// # Errors
///
/// Returns `Err(())` if the remote timestamp is >5s in the future (drift protection).
pub fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
    imp::update_hlc(remote_ts)
}

/// Reset for testing.
#[cfg(test)]
pub fn reset_for_testing() {
    imp::reset_for_testing();
}

#[cfg(target_arch = "wasm32")]
mod calimero_vm {
    use std::cell::RefCell;

    use calimero_sdk::env;

    use crate::logical_clock::{HybridTimestamp, HLC};
    use crate::store::Key;

    thread_local! {
        // Uses custom getrandom backend (see getrandom_impl.rs)
        // 5s drift protection (default 500ms is too strict for distributed systems)
        static HLC_INSTANCE: RefCell<HLC> = RefCell::new(
            crate::logical_clock::HLCBuilder::new()
                .with_max_delta(std::time::Duration::from_secs(5))
                .build()
        );
    }

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

    /// Return the executor id.
    pub(super) fn executor_id() -> [u8; 32] {
        env::executor_id()
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp.
    ///
    pub(super) fn time_now() -> u64 {
        env::time_now()
    }

    /// Get a new hybrid timestamp from the HLC
    pub(super) fn hlc_timestamp() -> HybridTimestamp {
        HLC_INSTANCE.with(|hlc| HybridTimestamp::from(hlc.borrow().new_timestamp()))
    }

    /// Update HLC with remote timestamp
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
        HLC_INSTANCE.with(|hlc| {
            hlc.borrow()
                .update_with_timestamp(remote_ts.inner())
                .map_err(|_| ())
        })
    }

    /// Resets the environment state for testing.
    #[cfg(test)]
    pub(super) fn reset_for_testing() {
        HLC_INSTANCE.with(|hlc| {
            *hlc.borrow_mut() = HLC::default();
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod mocked {
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rand::RngCore;

    use crate::logical_clock::{HybridTimestamp, HLC};
    use crate::store::{Key, MockedStorage, StorageAdaptor};

    thread_local! {
        static ROOT_HASH: RefCell<Option<[u8; 32]>> = const { RefCell::new(None) };
        // 5s drift protection (default 500ms is too strict for distributed systems)
        static HLC_INSTANCE: RefCell<HLC> = RefCell::new(
            crate::logical_clock::HLCBuilder::new()
                .with_max_delta(std::time::Duration::from_secs(5))
                .build()
        );
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

    /// Return the executor id (for testing, returns a fixed value).
    pub(super) const fn executor_id() -> [u8; 32] {
        [237; 32]
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

    /// Get a new hybrid timestamp from the HLC
    pub(super) fn hlc_timestamp() -> HybridTimestamp {
        HLC_INSTANCE.with(|hlc| HybridTimestamp::from(hlc.borrow().new_timestamp()))
    }

    /// Update HLC with remote timestamp
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
        HLC_INSTANCE.with(|hlc| {
            hlc.borrow()
                .update_with_timestamp(remote_ts.inner())
                .map_err(|_| ())
        })
    }

    /// Resets the environment state for testing.
    ///
    /// Clears the thread-local ROOT_HASH and HLC, allowing multiple commits
    /// in the same test execution context.
    #[cfg(test)]
    pub(super) fn reset_for_testing() {
        ROOT_HASH.with(|rh| {
            *rh.borrow_mut() = None;
        });
        HLC_INSTANCE.with(|hlc| {
            *hlc.borrow_mut() = HLC::default();
        });
    }
}
