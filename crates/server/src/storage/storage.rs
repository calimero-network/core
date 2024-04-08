use std::collections::BTreeMap;

pub type Key = Vec<u8>;
pub type Value = Vec<u8>;

pub enum AdminStore {
    Read(calimero_store::ReadOnlyStore),
    Write(calimero_store::TemporalStore),
}

impl Storage for AdminStore {
    fn get(&self, key: &calimero_runtime::store::Key) -> Option<Vec<u8>> {
        match self {
            Self::Read(store) => store.get(key).ok().flatten(),
            Self::Write(store) => store.get(key).ok().flatten(),
        }
    }

    fn set(
        &mut self,
        key: calimero_runtime::store::Key,
        value: calimero_runtime::store::Value,
    ) -> Option<calimero_runtime::store::Value> {
        match self {
            Self::Read(_) => unimplemented!("Can not write to read-only store."),
            Self::Write(store) => store.put(key, value),
        }
    }

    fn has(&self, key: &calimero_runtime::store::Key) -> bool {
        // todo! optimize to avoid eager reads
        match self {
            Self::Read(store) => store.get(key).ok().is_some(),
            Self::Write(store) => store.get(key).ok().is_some(),
        }
    }
}

pub trait Storage: Send {
    fn get(&self, key: &Key) -> Option<Value>;
    fn set(&mut self, key: Key, value: Value) -> Option<Value>;
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

    fn has(&self, key: &Key) -> bool {
        self.inner.contains_key(key)
    }
}
