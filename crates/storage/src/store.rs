//! Storage operations.

use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::env::{storage_read, storage_remove, storage_write};

/// A key for storage operations.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Key {
    /// An index key.
    Index(Id),

    /// An entry key.
    Entry(Id),
}

impl Key {
    /// Converts the key to a byte array.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut bytes = [0; 33];
        match *self {
            Self::Index(id) => {
                bytes[0] = 0;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
            Self::Entry(id) => {
                bytes[0] = 1;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
        }
        Sha256::digest(bytes).into()
    }
}

/// Determines where the ultimate storage system is located.
///
/// This trait is mainly used to allow for a different storage location to be
/// used for key operations during testing, such as modelling a foreign node's
/// data store.
///
pub(crate) trait StorageAdaptor {
    /// Reads data from persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key` - The key to read data from.
    ///
    fn storage_read(key: Key) -> Option<Vec<u8>>;

    /// Removes data from persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key` - The key to remove.
    ///
    fn storage_remove(key: Key) -> bool;

    /// Writes data to persistent storage.
    ///
    /// # Parameters
    ///
    /// * `key`   - The key to write data to.
    /// * `value` - The data to write.
    ///
    fn storage_write(key: Key, value: &[u8]) -> bool;
}

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
pub struct MainStorage;

impl StorageAdaptor for MainStorage {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        storage_read(key)
    }

    fn storage_remove(key: Key) -> bool {
        storage_remove(key)
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        storage_write(key, value)
    }
}

#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) use mocked::MockedStorage;

/// The mocked storage system.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) mod mocked {
    use core::cell::RefCell;
    use std::collections::BTreeMap;

    use super::{Key, StorageAdaptor};

    /// The scope of the storage system, which allows for multiple storage
    /// systems to be used in parallel.
    type Scope = usize;

    thread_local! {
        static STORAGE: RefCell<BTreeMap<(Scope, Key), Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
    }

    /// The mocked storage system.
    #[expect(clippy::redundant_pub_crate, reason = "Needed here")]
    pub(crate) struct MockedStorage<const SCOPE: usize>;

    impl<const SCOPE: usize> StorageAdaptor for MockedStorage<SCOPE> {
        fn storage_read(key: Key) -> Option<Vec<u8>> {
            STORAGE.with(|storage| storage.borrow().get(&(SCOPE, key)).cloned())
        }

        fn storage_remove(key: Key) -> bool {
            STORAGE.with(|storage| storage.borrow_mut().remove(&(SCOPE, key)).is_some())
        }

        fn storage_write(key: Key, value: &[u8]) -> bool {
            STORAGE.with(|storage| {
                storage
                    .borrow_mut()
                    .insert((SCOPE, key), value.to_vec())
                    .is_some()
            })
        }
    }
}
