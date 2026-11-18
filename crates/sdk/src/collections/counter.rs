//! `Counter`: a non-negative integer at `(name, "n")`.

use crate::app::StateField;
use crate::commons::{collection_key, KeyPath};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, Store};

/// A counter. `add` never conflicts; `increment` reads and rewrites the key, so it
/// serializes every call that uses it (D47). Use `env::unique_id()` for generated ids.
pub struct Counter<S: Store = DefaultStore> {
    name: KeyPath,
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
            name: KeyPath::root(name),
            store,
        }
    }

    fn storage_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    fn read(&self) -> Result<i128, StorageError> {
        match self.store.get(&self.storage_key()?) {
            Some(bytes) => value::int_from_slice(&bytes),
            None => Ok(0),
        }
    }

    /// The current value.
    pub fn get(&self) -> Result<u64, StorageError> {
        u64::try_from(self.read()?).map_err(|_| StorageError::BadCounter(self.name.label().into()))
    }

    /// Add one and return the new value. Reads and serializes the key; see the type docs.
    pub fn increment(&mut self) -> Result<u64, StorageError> {
        let next = self.read()? + 1;
        let out =
            u64::try_from(next).map_err(|_| StorageError::BadCounter(self.name.label().into()))?;
        let key = self.storage_key()?;
        self.store.put(&key, &value::int_to_vec(next)?);
        Ok(out)
    }

    /// Subtract one and return the new value; at zero it stays zero and writes nothing.
    /// Reads and serializes the key, as [`Counter::increment`] does.
    pub fn decrement(&mut self) -> Result<u64, StorageError> {
        let now = self.get()?;
        if now == 0 {
            return Ok(0);
        }
        let key = self.storage_key()?;
        self.store
            .put(&key, &value::int_to_vec(i128::from(now - 1))?);
        Ok(now - 1)
    }

    /// Set the counter back to zero (deletes its key).
    pub fn reset(&mut self) -> Result<(), StorageError> {
        let key = self.storage_key()?;
        self.store.delete(&key);
        Ok(())
    }

    /// Add `delta` without reading the counter (`storage.add`).
    pub fn add(&mut self, delta: i64) -> Result<(), StorageError> {
        let key = self.storage_key()?;
        self.store.add(&key, delta);
        Ok(())
    }
}

impl<S: Store> crate::collections::Nestable<S> for Counter<S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self { name: path, store }
    }
}
