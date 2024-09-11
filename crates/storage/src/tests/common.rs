use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::Store;
use tempfile::{tempdir, TempDir};

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
    let config = StoreConfig::new(
        temp_dir
            .path()
            .to_path_buf()
            .try_into()
            .expect("Invalid UTF-8 path"),
    );
    (
        Store::open::<RocksDB>(&config).expect("Could not create store"),
        temp_dir,
    )
}
