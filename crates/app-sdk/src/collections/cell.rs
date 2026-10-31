//! `Cell<T>`: one value at `(name, "v")` (v1's `LwwRegister`; the log orders every write).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::collection_key;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, Store};

/// One optional value: a later `set` replaces an earlier one.
pub struct Cell<T, S: Store = DefaultStore> {
    name: String,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T, S: Store> StateField for Cell<T, S> {
    const KIND: &'static str = "cell";
}

impl<T: Serialize + DeserializeOwned, S: Store + Default> Cell<T, S> {
    /// A cell on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> Cell<T, S> {
    /// A cell on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "v", &[])
    }

    /// The value; `None` before the first `set` and after `clear`.
    pub fn get(&self) -> Result<Option<T>, StorageError> {
        self.store
            .get(&self.storage_key()?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Set the value.
    pub fn set(&mut self, v: &T) -> Result<(), StorageError> {
        let key = self.storage_key()?;
        self.store.put(&key, &value::to_vec(v)?);
        Ok(())
    }

    /// Remove the value.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        let key = self.storage_key()?;
        self.store.delete(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    #[test]
    fn get_set_clear() {
        let store = MemStore::new();
        let mut c: Cell<String, MemStore> = Cell::with_store("c", store.clone());
        assert_eq!(c.get().unwrap(), None);
        c.set(&"one".into()).unwrap();
        c.set(&"two".into()).unwrap();
        assert_eq!(c.get().unwrap().as_deref(), Some("two"));
        assert_eq!(store.len(), 1);
        c.clear().unwrap();
        assert_eq!(c.get().unwrap(), None);
        assert!(store.is_empty());
    }

    #[test]
    fn two_cells_do_not_share_a_key() {
        let store = MemStore::new();
        let mut a: Cell<u32, MemStore> = Cell::with_store("a", store.clone());
        let b: Cell<u32, MemStore> = Cell::with_store("b", store);
        a.set(&1).unwrap();
        assert_eq!(b.get().unwrap(), None);
    }
}
