//! `Counter`: a non-negative integer at `(name, "n")`.

use crate::app::StateField;
use crate::encoding::key::{self, KeyPart};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, Store};

/// A counter. `add` never conflicts; `increment` reads and rewrites the key, so it
/// serializes every call that uses it (D47). Use `env::unique_id()` for generated ids.
pub struct Counter<S: Store = DefaultStore> {
    name: String,
    store: S,
}

impl<S: Store> StateField for Counter<S> {
    const KIND: &'static str = "counter";
}

impl<S: Store + Default> Counter<S> {
    /// A counter on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<S: Store> Counter<S> {
    /// A counter on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
        }
    }

    fn storage_key(&self) -> Result<Vec<u8>, StorageError> {
        let encoded = key::encode(&[KeyPart::Str(self.name.clone()), KeyPart::Str("n".into())]);
        check_key(&encoded)?;
        Ok(encoded)
    }

    fn read(&self) -> Result<i128, StorageError> {
        match self.store.get(&self.storage_key()?) {
            Some(bytes) => value::int_from_slice(&bytes),
            None => Ok(0),
        }
    }

    /// The current value.
    pub fn get(&self) -> Result<u64, StorageError> {
        u64::try_from(self.read()?).map_err(|_| StorageError::BadCounter(self.name.clone()))
    }

    /// Add one and return the new value. Reads and serializes the key; see the type docs.
    pub fn increment(&mut self) -> Result<u64, StorageError> {
        let next = self.read()? + 1;
        let out = u64::try_from(next).map_err(|_| StorageError::BadCounter(self.name.clone()))?;
        let key = self.storage_key()?;
        self.store.put(&key, &value::int_to_vec(next)?);
        Ok(out)
    }

    /// Add `delta` without reading the counter (`storage.add`).
    pub fn add(&mut self, delta: i64) -> Result<(), StorageError> {
        let key = self.storage_key()?;
        self.store.add(&key, delta);
        Ok(())
    }
}
