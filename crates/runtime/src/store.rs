use core::fmt::Debug;
use std::{
    collections::{btree_map::Entry as BTreeMapEntry, BTreeMap, HashSet},
    mem,
};

use thiserror::Error;

pub type Id = [u8; 32];
pub type Value = Vec<u8>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("id points at a collection, not a value")]
    NotAValue,
    #[error("id points at a value, not a collection")]
    NotACollection,
    #[error("collection is not empty")]
    CollectionNotEmpty,
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

pub trait Storage: Debug {
    fn create(&mut self, is_collection: bool) -> Result<Id, StorageError>;
    fn exists(&self, key: &Id) -> Result<bool, StorageError>;
    fn read(&self, key: &Id) -> Result<Option<Value>, StorageError>;
    fn write(&mut self, key: Id, value: Value) -> Result<Option<Value>, StorageError>;
    fn remove(&mut self, key: &Id) -> Result<Option<Value>, StorageError>;
    fn adopt(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError>;
    fn orphan(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError>;
}

#[derive(Debug)]
enum Entry {
    Bytes(Vec<u8>),
    Collection(HashSet<Id>),
}

#[derive(Debug, Default)]
pub struct InMemoryStorage {
    inner: BTreeMap<Id, Entry>,
}

impl Storage for InMemoryStorage {
    fn create(&mut self, is_collection: bool) -> Result<Id, StorageError> {
        let id = loop {
            let id = rand::random::<Id>();

            if !self.inner.contains_key(&id) {
                break id;
            }
        };

        let entry = if is_collection {
            Entry::Collection(HashSet::default())
        } else {
            Entry::Bytes(vec![])
        };

        let _ignored = self.inner.insert(id, entry);

        Ok(id)
    }

    fn exists(&self, key: &Id) -> Result<bool, StorageError> {
        Ok(self.inner.contains_key(key))
    }

    fn read(&self, key: &Id) -> Result<Option<Value>, StorageError> {
        let bytes = match self.inner.get(key) {
            Some(Entry::Bytes(bytes)) => Some(bytes.clone()),
            _ => None,
        };

        Ok(bytes)
    }

    fn write(&mut self, key: Id, value: Value) -> Result<Option<Value>, StorageError> {
        let old = match self.inner.entry(key) {
            BTreeMapEntry::Vacant(entry) => {
                let _ = entry.insert(Entry::Bytes(value));
                None
            }
            BTreeMapEntry::Occupied(mut entry) => match entry.get_mut() {
                Entry::Collection(_) => return Err(StorageError::NotAValue),
                Entry::Bytes(bytes) => Some(mem::replace(bytes, value)),
            },
        };

        Ok(old)
    }

    fn remove(&mut self, key: &Id) -> Result<Option<Value>, StorageError> {
        let old = match self.inner.entry(*key) {
            BTreeMapEntry::Vacant(_) => None,
            BTreeMapEntry::Occupied(occupied_entry) => {
                let entry = occupied_entry.get();

                if let Entry::Collection(collection) = entry {
                    if !collection.is_empty() {
                        return Err(StorageError::CollectionNotEmpty);
                    }
                }

                Some(occupied_entry.remove())
            }
        };

        Ok(old.and_then(|entry| match entry {
            Entry::Bytes(bytes) => Some(bytes),
            Entry::Collection(_) => None,
        }))
    }

    fn adopt(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError> {
        if !self.inner.contains_key(&key) {
            return Ok(false);
        }

        let Some(entry) = self.inner.get_mut(parent) else {
            return Ok(false);
        };

        let Entry::Collection(collection) = entry else {
            return Err(StorageError::NotACollection);
        };

        let _ = collection.insert(key);

        Ok(true)
    }

    fn orphan(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError> {
        if !self.inner.contains_key(&key) {
            return Ok(false);
        }

        let Some(entry) = self.inner.get_mut(parent) else {
            return Ok(false);
        };

        let Entry::Collection(collection) = entry else {
            return Err(StorageError::NotACollection);
        };

        let _ = collection.remove(&key);

        Ok(true)
    }
}
