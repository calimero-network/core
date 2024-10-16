use core::cell::RefCell;
use std::collections::HashMap;
use std::thread_local;

thread_local! {
    static MOCK_STORAGE: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
}

pub fn mock_storage_read(key: &[u8]) -> Option<Vec<u8>> {
    MOCK_STORAGE.with(|storage| storage.borrow().get(key).cloned())
}

pub fn mock_storage_remove(key: &[u8]) -> bool {
    MOCK_STORAGE.with(|storage| storage.borrow_mut().remove(key).is_some())
}

pub fn mock_storage_write(key: &[u8], value: &[u8]) -> bool {
    MOCK_STORAGE.with(|storage| {
        storage
            .borrow_mut()
            .insert(key.to_vec(), value.to_vec())
            .is_some()
    })
}
