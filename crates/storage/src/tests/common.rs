use std::sync::LazyLock;

use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::Store;
use tempfile::{tempdir, TempDir};

use crate::address::Id;

/// A set of non-empty test UUIDs.
pub const TEST_UUID: [[u8; 16]; 5] = [
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [2, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [3, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [4, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [5, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
];

/// A set of non-empty test IDs.
pub static TEST_ID: LazyLock<[Id; 5]> = LazyLock::new(|| {
    [
        Id::from(TEST_UUID[0]),
        Id::from(TEST_UUID[1]),
        Id::from(TEST_UUID[2]),
        Id::from(TEST_UUID[3]),
        Id::from(TEST_UUID[4]),
    ]
});

/// Creates a new temporary store for testing.
///
/// This function creates a new temporary directory and opens a new store in it,
/// returning the store and the directory. The directory and its test data will
/// be cleaned up when the test completes.
///
pub fn create_test_store() -> (Store, TempDir) {
    // Note: It would be nice to have a way to test against an in-memory store,
    // but InMemoryDB is not integrated with Store and there's currently no way
    // to support both. It may be that the Database trait is later applied to
    // InMemoryDB as well as RocksDB, in which case the storage Interface could
    // be changed to work against Database implementations and not just Store.
    let temp_dir = tempdir().expect("Could not create temp dir");
    let config = StoreConfig {
        path: temp_dir
            .path()
            .to_path_buf()
            .try_into()
            .expect("Invalid UTF-8 path"),
    };
    (
        Store::open::<RocksDB>(&config).expect("Could not create store"),
        temp_dir,
    )
}
