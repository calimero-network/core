use std::cell::RefCell;
use std::collections::HashMap;

use crate::index::Index;
use crate::interface::{MainInterface, StorageAdaptor};

thread_local! {
    static FOREIGN_STORAGE: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
}

pub(crate) type ForeignInterface = MainInterface<ForeignStorage>;

#[expect(dead_code, reason = "Here to be used by tests")]
pub(crate) type ForeignIndex = Index<ForeignStorage>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub(crate) struct ForeignStorage;

impl StorageAdaptor for ForeignStorage {
    fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
        FOREIGN_STORAGE.with(|storage| storage.borrow().get(key).cloned())
    }

    fn storage_remove(key: &[u8]) -> bool {
        FOREIGN_STORAGE.with(|storage| storage.borrow_mut().remove(key).is_some())
    }

    fn storage_write(key: &[u8], value: &[u8]) -> bool {
        FOREIGN_STORAGE.with(|storage| {
            storage
                .borrow_mut()
                .insert(key.to_vec(), value.to_vec())
                .is_some()
        })
    }
}
