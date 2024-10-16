//! Environment bindings for the storage crate.

#[cfg(target_arch = "wasm32")]
use calimero_vm as imp;

#[cfg(not(target_arch = "wasm32"))]
use mocked as imp;

/// Reads data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to read data from.
///
#[must_use]
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    imp::storage_read(key)
}

/// Removes data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to remove.
///
#[must_use]
pub fn storage_remove(key: &[u8]) -> bool {
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
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
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
///
#[must_use]
pub fn time_now() -> u64 {
    imp::time_now()
}

#[cfg(target_arch = "wasm32")]
mod calimero_vm {
    use calimero_sdk::env;

    /// Reads data from persistent storage.
    ///
    pub(super) fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
        env::storage_read(key)
    }

    /// Removes data from persistent storage.
    ///
    pub(super) fn storage_remove(key: &[u8]) -> bool {
        env::storage_remove(key)
    }

    /// Writes data to persistent storage.
    ///
    pub(super) fn storage_write(key: &[u8], value: &[u8]) -> bool {
        env::storage_write(key, value)
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        env::random_bytes(buf)
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
    use core::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rand::RngCore;

    thread_local! {
        static STORAGE: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
    }

    /// Reads data from persistent storage.
    ///
    pub(super) fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
        STORAGE.with(|storage| storage.borrow().get(key).cloned())
    }

    /// Removes data from persistent storage.
    ///
    pub(super) fn storage_remove(key: &[u8]) -> bool {
        STORAGE.with(|storage| storage.borrow_mut().remove(key).is_some())
    }

    /// Writes data to persistent storage.
    ///
    pub(super) fn storage_write(key: &[u8], value: &[u8]) -> bool {
        STORAGE.with(|storage| {
            storage
                .borrow_mut()
                .insert(key.to_vec(), value.to_vec())
                .is_some()
        })
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        rand::thread_rng().fill_bytes(buf);
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
