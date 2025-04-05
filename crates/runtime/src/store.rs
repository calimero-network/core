use core::fmt::Debug;
use std::collections::btree_map::IntoIter;
use std::collections::BTreeMap;

use calimero_primitives::reflect::Reflect;

pub type Key = Vec<u8>;
pub type Value = Vec<u8>;

pub trait Storage: Reflect {
    fn get(&self, key: &Key) -> Option<Value>;
    fn set(&mut self, key: Key, value: Value) -> Option<Value>;
    fn remove(&mut self, key: &Key) -> Option<Vec<u8>>;
    fn has(&self, key: &Key) -> bool;
}

#[derive(Debug, Default)]
pub struct InMemoryStorage {
    inner: BTreeMap<Key, Value>,
}

impl Storage for InMemoryStorage {
    fn get(&self, key: &Key) -> Option<Value> {
        self.inner.get(key).cloned()
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        self.inner.insert(key, value)
    }

    // todo! revisit this, should we return the value by default?
    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        self.inner.remove(key)
    }

    fn has(&self, key: &Key) -> bool {
        self.inner.contains_key(key)
    }
}

impl IntoIterator for InMemoryStorage {
    type Item = (Key, Value);

    type IntoIter = IntoIter<Key, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
