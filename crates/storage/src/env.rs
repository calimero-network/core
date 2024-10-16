use calimero_sdk::env::{random_bytes, storage_read, storage_remove, storage_write, time_now};

/// The main storage system.
///
/// This is the default storage system, and is used for the main storage
/// operations in the system. It uses the environment's storage system to
/// perform the actual storage operations.
///
/// It is the only one intended for use in production, with other options being
/// implemented internally for testing purposes.
///
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub struct CalimeroVM;

impl Environment for CalimeroVM {
    fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
        storage_read(key)
    }

    fn storage_remove(key: &[u8]) -> bool {
        storage_remove(key)
    }

    fn storage_write(key: &[u8], value: &[u8]) -> bool {
        storage_write(key, value)
    }

    fn random_bytes(buf: &mut [u8]) {
        random_bytes(buf)
    }

    fn time_now() -> u64 {
        time_now()
    }
}

/// Determines where the ultimate storage system is located.
///
/// This trait is mainly used to allow for a different storage location to be
/// used for key operations during testing, such as modelling a foreign node's
/// data store.
///
pub trait Environment {
    /// Reads data from persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key` - The key to read data from.
    ///
    fn storage_read(key: &[u8]) -> Option<Vec<u8>>;

    /// Removes data from persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key` - The key to remove.
    ///
    fn storage_remove(key: &[u8]) -> bool;

    /// Writes data to persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key`   - The key to write data to.
    /// * `value` - The data to write.
    ///
    fn storage_write(key: &[u8], value: &[u8]) -> bool;

    /// Fill the buffer with random bytes.
    ///
    /// # Parameters
    ///
    /// * `buf` - The buffer to fill with random bytes.
    ///
    fn random_bytes(buf: &mut [u8]);

    /// Get the current time.
    ///
    fn time_now() -> u64;
}
