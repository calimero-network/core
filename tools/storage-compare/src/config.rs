//! Production `State` options at HEAD.
use rocksdb::{Cache, Options};
pub const LABEL: &str = "after (HEAD)";
pub fn state_options() -> Options {
    let cache = Cache::new_lru_cache(128 * 1024 * 1024);
    let table = calimero_store_rocksdb::table_options(&cache);
    calimero_store_rocksdb::column_options(&table).expect("column options")
}
