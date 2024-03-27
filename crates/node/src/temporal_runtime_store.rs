pub enum TemporalRuntimeStore {
    Read(calimero_store::ReadOnlyStore),
    Write(calimero_store::TemporalStore),
}

impl calimero_runtime::store::Storage for TemporalRuntimeStore {
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
