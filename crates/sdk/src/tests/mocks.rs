use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

static MOCK_STORAGE: OnceLock<RwLock<HashMap<Vec<u8>, Vec<u8>>>> = OnceLock::new();

fn get_mock_storage() -> &'static RwLock<HashMap<Vec<u8>, Vec<u8>>> {
    MOCK_STORAGE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn mock_storage_read(key: &[u8]) -> Option<Vec<u8>> {
    get_mock_storage().read().unwrap().get(key).cloned()
}

pub fn mock_storage_remove(key: &[u8]) -> bool {
    get_mock_storage().write().unwrap().remove(key).is_some()
}

pub fn mock_storage_write(key: &[u8], value: &[u8]) -> bool {
    get_mock_storage()
        .write()
        .unwrap()
        .insert(key.to_vec(), value.to_vec())
        .is_some()
}
