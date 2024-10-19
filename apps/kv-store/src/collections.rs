use calimero_storage::{
    address::{Path, PathError},
    entities::{Data, Element},
    interface::{Interface, StorageError},
};
use calimero_storage_macros::{AtomicUnit, Collection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    StorageError(#[from] StorageError),
    #[error(transparent)]
    PathError(#[from] PathError),
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(255)]
#[root]
pub struct Map {
    entries: Entries,
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry)]
pub struct Entries;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(254)]
pub struct Entry {
    key: String,
    value: String,
    #[storage]
    storage: Element,
}

impl Map {
    pub fn new(path: &Path) -> Result<Self, Error> {
        let mut this = Self {
            entries: Entries,
            storage: Element::new(path),
        };

        Interface::save(&mut this)?;

        Ok(this)
    }

    pub fn set(&mut self, key: String, value: String) -> Result<Option<String>, Error> {
        let previous = self.get(&key)?;

        let path = self.path();
        // fixme! Reusing the Map's path for now. We "could" concatenate, but it's
        // fixme! non-trivial and currently non-functional, so it's been left out

        let storage = Element::new(&path);
        // fixme! This uses a random id for the map's entries, which will impair
        // fixme! perf on the lookup, as we'd have to fetch and look through all
        // fixme! entries to find the one that matches the key we're looking for
        // fixme! ideally, the Id should be defined as hash(concat(map_id, key))
        // fixme! which will save on map-wide lookups, getting the item directly

        Interface::add_child_to(
            self.storage.id(),
            &mut self.entries,
            &mut Entry {
                key,
                value,
                storage,
            },
        )?;

        Ok(previous)
    }

    pub fn entries(&self) -> Result<impl Iterator<Item = (String, String)>, Error> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| (entry.key, entry.value)))
    }

    pub fn len(&self) -> Result<usize, Error> {
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, Error> {
        for (key_, value) in self.entries()? {
            if key_ == key {
                return Ok(Some(value));
            }
        }

        Ok(None)
    }

    pub fn remove(&mut self, key: &str) -> Result<Option<String>, Error> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        let entry = entries.into_iter().find(|entry| entry.key == key);

        if let Some(entry) = &entry {
            Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(entry.map(|entry| entry.value))
    }

    pub fn clear(&mut self) -> Result<(), Error> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}
