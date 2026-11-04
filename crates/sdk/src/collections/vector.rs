//! `Vector<T>`: elements at `(name, "e", position)` for positions `0..len`, the length at
//! `(name, "n")` (v1's `Vector`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{clear_prefix, collection_key, read_count, read_page};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, Store};
use crate::table::{FindOpts, Page};

/// A list of values by position; it grows and shrinks at the end.
pub struct Vector<T, S: Store = DefaultStore> {
    name: String,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T, S: Store> StateField for Vector<T, S> {
    const KIND: &'static str = "vector";
}

impl<T: Serialize + DeserializeOwned, S: Store + Default> Vector<T, S> {
    /// A vector on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> Vector<T, S> {
    /// A vector on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self, index: u64) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "e", &[KeyPart::Int(index.into())])
    }

    fn base(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "e", &[])
    }

    fn len_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    /// How many elements there are: one read of the stored length.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.len_key()?, &self.name)
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    /// Append `v`; returns its position.
    pub fn push(&mut self, v: &T) -> Result<u64, StorageError> {
        let index = self.len()?;
        let key = self.storage_key(index)?;
        self.store.put(&key, &value::to_vec(v)?);
        self.store.add(&self.len_key()?, 1);
        Ok(index)
    }

    /// The element at `index`; `None` past the end.
    pub fn get(&self, index: u64) -> Result<Option<T>, StorageError> {
        self.store
            .get(&self.storage_key(index)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Replace the element at `index`, returning the old one; `OutOfBounds` past the end.
    pub fn set(&mut self, index: u64, v: &T) -> Result<T, StorageError> {
        let len = self.len()?;
        if index >= len {
            return Err(StorageError::OutOfBounds { index, len });
        }
        let key = self.storage_key(index)?;
        let old = self
            .store
            .get(&key)
            .ok_or(StorageError::Dangling)
            .and_then(|b| value::from_slice(&b))?;
        self.store.put(&key, &value::to_vec(v)?);
        Ok(old)
    }

    /// Remove and return the last element.
    pub fn pop(&mut self) -> Result<Option<T>, StorageError> {
        let len = self.len()?;
        let Some(last) = len.checked_sub(1) else {
            return Ok(None);
        };
        let key = self.storage_key(last)?;
        let old = self
            .store
            .get(&key)
            .ok_or(StorageError::Dangling)
            .and_then(|b| value::from_slice(&b))?;
        self.store.delete(&key);
        self.store.add(&self.len_key()?, -1);
        Ok(Some(old))
    }

    /// Elements in position order (last first with `reverse`), one page of at most 1,000;
    /// `from` and `to` take positions as `KeyPart::Int`.
    pub fn page(&self, opts: FindOpts) -> Result<Page<T>, StorageError> {
        let (entries, next) = read_page(&self.store, &self.base()?, &opts)?;
        let items = entries
            .iter()
            .map(|(_, bytes)| value::from_slice(bytes))
            .collect::<Result<_, _>>()?;
        Ok(Page { items, next })
    }

    /// Remove every element, a batch at a time; returns how many there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let base = self.base()?;
        let count = self.len_key()?;
        let removed = clear_prefix(&mut self.store, &base);
        self.store.delete(&count);
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    fn vector() -> (Vector<String, MemStore>, MemStore) {
        let store = MemStore::new();
        (Vector::with_store("v", store.clone()), store)
    }

    #[test]
    fn push_get_set_pop() {
        let (mut v, _) = vector();
        assert_eq!(v.pop().unwrap(), None);
        assert_eq!(v.push(&"a".into()).unwrap(), 0);
        assert_eq!(v.push(&"b".into()).unwrap(), 1);
        assert_eq!(v.len().unwrap(), 2);
        assert_eq!(v.get(1).unwrap().as_deref(), Some("b"));
        assert_eq!(v.get(2).unwrap(), None);
        assert_eq!(v.set(0, &"A".into()).unwrap(), "a");
        assert_eq!(
            v.set(2, &"c".into()).unwrap_err(),
            StorageError::OutOfBounds { index: 2, len: 2 }
        );
        assert_eq!(v.pop().unwrap().as_deref(), Some("b"));
        assert_eq!(v.len().unwrap(), 1);
        assert_eq!(v.push(&"c".into()).unwrap(), 1);
        assert_eq!(v.page(FindOpts::default()).unwrap().items, ["A", "c"]);
    }

    #[test]
    fn positions_sort_numerically_past_nine() {
        let (mut v, _) = vector();
        for i in 0..12 {
            let _ = v.push(&format!("{i}")).unwrap();
        }
        let items = v.page(FindOpts::default()).unwrap().items;
        assert_eq!(items[10], "10");
        let tail = v
            .page(FindOpts::default().from(vec![KeyPart::Int(9)]))
            .unwrap()
            .items;
        assert_eq!(tail, ["9", "10", "11"]);
        let back = v.page(FindOpts::default().reverse().limit(2)).unwrap();
        assert_eq!(back.items, ["11", "10"]);
    }

    #[test]
    fn the_length_is_kept_with_add_and_clear_resets_it() {
        let (mut v, store) = vector();
        for i in 0..1_100 {
            let _ = v.push(&format!("{i}")).unwrap();
        }
        assert_eq!(store.op_counts(&v.len_key().unwrap()), (0, 1_100));
        assert_eq!(v.clear().unwrap(), 1_100);
        assert_eq!(v.len().unwrap(), 0);
        assert!(store.is_empty());
        assert_eq!(v.push(&"again".into()).unwrap(), 0);
    }
}
