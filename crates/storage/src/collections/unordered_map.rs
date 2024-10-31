use core::borrow::Borrow;
use core::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

// fixme! macro expects `calimero_storage` to be in deps
use crate as calimero_storage;
use crate::address::{Id, Path};
use crate::collections::error::StoreError;
use crate::entities::{Data, Element};
use crate::interface::Interface;
use crate::{AtomicUnit, Collection};

/// A map collection that stores key-value pairs.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(255)]
#[root]
pub struct UnorderedMap<K, V> {
    /// The id used for the map's entries.
    id: Id,
    /// The entries in the map.
    entries: Entries<K, V>,
    /// The storage element for the map.
    #[storage]
    storage: Element,
}

/// A collection of entries in a map.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<K, V>)]
struct Entries<K, V> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<(K, V)>,
}

/// An entry in a map.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(254)]
pub struct Entry<K, V> {
    /// The key for the entry.
    key: K,
    /// The value for the entry.
    value: V,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

impl<
        K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
        V: BorshSerialize + BorshDeserialize,
    > UnorderedMap<K, V>
{
    /// Create a new map collection.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn new() -> Result<Self, StoreError> {
        let id = Id::random();
        let mut this = Self {
            id: id,
            entries: Entries::default(),
            storage: Element::new(&Path::new(format!("::unused::map::{id}::path"))?, Some(id)),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Compute the ID for a key.
    fn compute_id(&self, key: &[u8]) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
        hasher.update(key);
        Id::new(hasher.finalize().into())
    }

    /// Get the raw entry for a key in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    fn get_raw<Q>(&self, key: &Q) -> Result<Option<Entry<K, V>>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        Ok(Interface::find_by_id::<Entry<K, V>>(
            self.compute_id(key.as_ref()),
        )?)
    }

    /// Insert a key-value pair into the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, key: K, value: V) -> Result<bool, StoreError> {
        if let Some(mut entry) = self.get_raw(&key)? {
            entry.value = value;
            // has to be called to update the entry
            entry.element_mut().update();
            let _ = Interface::save(&mut entry)?;
            return Ok(false);
        } else {
            let path = self.path();
            let storage = Element::new(&path, Some(self.compute_id(key.as_ref())));
            let _ = Interface::add_child_to(
                self.storage.id(),
                &mut self.entries,
                &mut Entry {
                    key,
                    value,
                    storage,
                },
            )?;
        }

        Ok(true)
    }

    /// Get an iterator over the entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)>, StoreError> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| (entry.key, entry.value)))
    }

    /// Get the number of entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn len(&self) -> Result<usize, StoreError> {
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
    }

    /// Get the value for a key in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn get<Q>(&self, key: &Q) -> Result<Option<V>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let entry = Interface::find_by_id::<Entry<K, V>>(self.compute_id(key.as_ref()))?;
        let value = entry.map(|e| e.value);

        Ok(value)
    }

    /// Check if the map contains a key.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn contains<Q>(&self, key: &Q) -> Result<bool, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        Ok(self.get_raw(key)?.is_some())
    }

    /// Remove a key from the map, returning the value at the key if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn remove<Q>(&mut self, key: &Q) -> Result<bool, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let entry = Element::new(&self.path(), Some(self.compute_id(key.as_ref())));

        Ok(Interface::remove_child_from(
            self.id(),
            &mut self.entries,
            entry.id(),
        )?)
    }

    /// Clear the map, removing all entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn clear(&mut self) -> Result<(), StoreError> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::unordered_map::UnorderedMap;

    #[test]
    fn test_unordered_map_basic_operations() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed"));

        assert_eq!(
            map.get("key").expect("get failed"),
            Some("value".to_string())
        );
        assert_ne!(
            map.get("key").expect("get failed"),
            Some("value2".to_string())
        );

        assert!(!map
            .insert("key".to_string(), "value2".to_string())
            .expect("insert failed"));
        assert!(map
            .insert("key2".to_string(), "value".to_string())
            .expect("insert failed"));

        assert_eq!(
            map.get("key").expect("get failed"),
            Some("value2".to_string())
        );
        assert_eq!(
            map.get("key2").expect("get failed"),
            Some("value".to_string())
        );

        assert_eq!(map.remove("key").expect("error while removing key"), true);
        assert_eq!(map.remove("key").expect("error while removing key"), false);

        assert_eq!(map.get("key").expect("get failed"), None);
    }

    #[test]
    fn test_unordered_map_insert_and_get() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed"));
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed"));

        assert_eq!(
            map.get("key1").expect("get failed"),
            Some("value1".to_string())
        );
        assert_eq!(
            map.get("key2").expect("get failed"),
            Some("value2".to_string())
        );
    }

    #[test]
    fn test_unordered_map_update_value() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed"));
        assert!(!map
            .insert("key".to_string(), "new_value".to_string())
            .expect("insert failed"));

        assert_eq!(
            map.get("key").expect("get failed"),
            Some("new_value".to_string())
        );
    }

    #[test]
    fn test_remove() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed"));

        assert_eq!(map.remove("key").expect("remove failed"), true);
        assert_eq!(map.get("key").expect("get failed"), None);
    }

    #[test]
    fn test_clear() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed"));
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed"));

        map.clear().expect("clear failed");

        assert_eq!(map.get("key1").expect("get failed"), None);
        assert_eq!(map.get("key2").expect("get failed"), None);
    }

    #[test]
    fn test_unordered_map_len() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert_eq!(map.len().expect("len failed"), 0);

        assert!(map
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed"));
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed"));
        assert!(!map
            .insert("key2".to_string(), "value3".to_string())
            .expect("insert failed"));

        assert_eq!(map.len().expect("len failed"), 2);

        assert_eq!(map.remove("key1").expect("remove failed"), true);

        assert_eq!(map.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_map_contains() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed"));

        assert_eq!(map.contains("key").expect("contains failed"), true);
        assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_map_entries() {
        let mut map = UnorderedMap::<String, String>::new().expect("failed to create map");

        assert!(map
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed"));
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed"));
        assert!(!map
            .insert("key2".to_string(), "value3".to_string())
            .expect("insert failed"));

        let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&("key1".to_string(), "value1".to_string())));
        assert!(entries.contains(&("key2".to_string(), "value3".to_string())));
    }
}
