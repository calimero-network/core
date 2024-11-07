//! High-level data structures for storage.

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

pub mod unordered_map;
pub use unordered_map::UnorderedMap;
pub mod unordered_set;
pub use unordered_set::UnorderedSet;
pub mod vector;
pub use vector::Vector;
pub mod error;
pub use error::StoreError;

use crate as calimero_storage;
use crate::address::{Id, Path};
use crate::entities::{Data, Element};
use crate::interface::{Interface, StorageError};
use crate::{AtomicUnit, Collection};

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(255)]
#[root]
struct Collection<T> {
    /// The entries in the collection.
    #[collection]
    entries: Entries<T>,
    /// The storage element for the map.
    #[storage]
    storage: Element,
}

/// A collection of entries in a map.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<T>)]
struct Entries<T> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<T>,
}

/// An entry in a map.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(254)]
struct Entry<T> {
    /// The item in the entry.
    item: T,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

#[derive(Debug)]
struct CollectionMut<'a, T: BorshSerialize + BorshDeserialize> {
    collection: &'a mut Collection<T>,
}

#[derive(Debug)]
struct EntryMut<'a, T: BorshSerialize + BorshDeserialize> {
    collection: CollectionMut<'a, T>,
    entry: Entry<T>,
}

type StoreResult<T> = std::result::Result<T, StoreError>;

impl<T: BorshSerialize + BorshDeserialize> Collection<T> {
    /// Creates a new collection.
    fn new() -> Self {
        let mut this = Self {
            entries: Entries { _priv: PhantomData },
            storage: Element::new(&Path::new("::unused").expect("valid path"), None),
        };

        let _ = Interface::save(&mut this).expect("save collection");

        this
    }

    /// Inserts an item into the collection.
    fn insert(&mut self, id: Option<Id>, item: T) -> StoreResult<()> {
        let path = self.path();

        let mut collection = CollectionMut::new(self);

        collection.insert(Entry {
            item,
            storage: Element::new(&path, id),
        })?;

        Ok(())
    }

    fn get(&self, id: Id) -> StoreResult<Option<T>> {
        let entry = Interface::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| entry.item))
    }

    fn get_mut(&mut self, id: Id) -> StoreResult<Option<EntryMut<'_, T>>> {
        let entry = Interface::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| EntryMut {
            collection: CollectionMut::new(self),
            entry,
        }))
    }

    fn entries(&self) -> StoreResult<impl ExactSizeIterator<Item = StoreResult<T>>> {
        let children = Interface::child_info_for(self.id(), &self.entries)?;

        let iter = children.into_iter().map(|child| {
            let id = child.id();

            let entry = Interface::find_by_id::<Entry<_>>(id)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(id)))?;

            Ok(entry.item)
        });

        Ok(iter)
    }

    fn clear(&mut self) -> StoreResult<()> {
        let mut collection = CollectionMut::new(self);

        collection.clear()?;

        Ok(())
    }
}

impl<T> EntryMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn remove(mut self) -> StoreResult<T> {
        let old = self
            .collection
            .get(self.entry.id())?
            .ok_or(StoreError::StorageError(StorageError::NotFound(
                self.entry.id(),
            )))?;

        let _ = Interface::remove_child_from(
            self.collection.id(),
            &mut self.collection.entries,
            self.entry.id(),
        )?;

        Ok(old)
    }
}

impl<T> Deref for EntryMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.entry.item
    }
}

impl<T> DerefMut for EntryMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry.item
    }
}

impl<T> Drop for EntryMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn drop(&mut self) {
        self.entry.element_mut().update();
        let _ignored = Interface::save(&mut self.entry);
    }
}

impl<'a, T> CollectionMut<'a, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn new(collection: &'a mut Collection<T>) -> Self {
        Self { collection }
    }

    fn insert(&mut self, item: Entry<T>) -> StoreResult<()> {
        let mut item = item;

        let _ = Interface::add_child_to(
            self.collection.id(),
            &mut self.collection.entries,
            &mut item,
        )?;

        Ok(())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let children =
            Interface::child_info_for(self.collection.id(), &mut self.collection.entries)?;

        for child in children {
            let _ = Interface::remove_child_from(
                self.collection.id(),
                &mut self.collection.entries,
                child.id(),
            )?;
        }

        Ok(())
    }
}

impl<T> Deref for CollectionMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    type Target = Collection<T>;

    fn deref(&self) -> &Self::Target {
        self.collection
    }
}

impl<T> DerefMut for CollectionMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.collection
    }
}

impl<T> Drop for CollectionMut<'_, T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn drop(&mut self) {
        self.collection.element_mut().update();
        let _ignored = Interface::save(self.collection);
    }
}
