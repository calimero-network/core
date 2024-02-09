use std::collections::HashMap;

pub trait Storage: Send {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
    fn set(&mut self, key: &[u8], value: &[u8]) -> Option<Vec<u8>>;
    // fn remove(&mut self, key: &[u8]);
    fn has(&self, key: &[u8]) -> bool;
}

#[derive(Debug, Default)]
pub struct InMemoryStorage {
    inner: HashMap<Vec<u8>, Vec<u8>>,
}

impl Storage for InMemoryStorage {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.get(key).cloned()
    }

    fn set(&mut self, key: &[u8], value: &[u8]) -> Option<Vec<u8>> {
        self.inner.insert(key.to_vec(), value.to_vec())
    }

    // todo! revisit this, should we return the value by default?
    // fn remove(&mut self, key: &[u8]) {
    //     self.inner.remove(key);
    // }

    fn has(&self, key: &[u8]) -> bool {
        self.inner.contains_key(key)
    }
}
