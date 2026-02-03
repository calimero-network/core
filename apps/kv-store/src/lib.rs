#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::unordered_map::Entry;
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap, UnorderedSet, Vector};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    /// Key-value pairs stored in the store
    items: UnorderedMap<String, LwwRegister<String>>,
    /// Total number of operations performed
    operation_count: Counter,
    /// History of operations (last 100 entries)
    /// Using LwwRegister<String> so each entry can be independently updated
    operation_history: Vector<LwwRegister<String>>,
    /// Tags associated with keys
    tags: UnorderedSet<String>,
    /// Store metadata
    metadata: LwwRegister<String>,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        // Use the auto-generated Default implementation which uses field names
        KvStore::default()
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        let was_update = self.items.contains(&key)?;

        if was_update {
            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
        } else {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        self.items.insert(key.clone(), value.clone().into())?;

        // Increment operation counter
        self.operation_count.increment()?;

        // Add to history (keep last 100 entries)
        let history_entry = if was_update {
            format!("Updated: {} = {}", key, value)
        } else {
            format!("Inserted: {} = {}", key, value)
        };
        self.operation_history
            .push(LwwRegister::new(history_entry))?;

        // Trim history to last 100 entries (pop from front)
        while self.operation_history.len()? > 100 {
            // Vector doesn't have remove, so we'll just limit on read
            // For now, we'll keep all entries and limit in get_operation_history
            break;
        }

        Ok(())
    }

    /// Updates a value only if the key already exists, using in-place mutation.
    ///
    /// This demonstrates the `get_mut` API which allows modifying the value
    /// without a read-modify-write cycle. The change is automatically persisted
    /// with a new timestamp when the guard is dropped.
    pub fn update_if_exists(&mut self, key: String, value: String) -> app::Result<bool> {
        app::log!("Updating if exists: {:?} -> {:?}", key, value);

        if let Some(mut v) = self.items.get_mut(&key)? {
            // Modifying the LwwRegister via the guard
            // This updates the value and timestamp in-place
            v.set(value.clone());

            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
            return Ok(true);
        }

        Ok(false)
    }

    /// Gets a value, inserting it if it doesn't exist.
    ///
    /// This demonstrates the `entry` API combined with `or_insert`.
    /// We pattern match to check existence for the event, then use the convenience method.
    pub fn get_or_insert(&mut self, key: String, value: String) -> app::Result<String> {
        app::log!("Get or insert: {:?} -> {:?}", key, value);

        let entry = self.items.entry(key.clone())?;

        // Check if vacant to emit event, without consuming the entry
        if let Entry::Vacant(_) = &entry {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        // Use the high-level API to handle the insertion or retrieval
        let val = entry.or_insert(LwwRegister::new(value))?;

        Ok(val.get().clone())
    }

    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");

        Ok(self
            .items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");

        Ok(self.items.len()?)
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn get_unchecked(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key without checking: {:?}", key);

        // this panics, which we do not recommend
        Ok(self.items.get(key)?.expect("key not found").get().clone())
    }

    pub fn get_result(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key, possibly failing: {:?}", key);

        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };

        Ok(value)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);

        app::emit!(Event::Removed { key });

        let result = self.items.remove(key)?.map(|v| v.get().clone());

        // Increment operation counter
        if result.is_some() {
            self.operation_count.increment()?;

            // Add to history
            let history_entry = format!("Removed: {}", key);
            self.operation_history
                .push(LwwRegister::new(history_entry))?;

            // History is limited to last 100 entries when reading (see get_operation_history)
        }

        Ok(result)
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear()?;

        // Increment operation counter
        self.operation_count.increment()?;

        // Add to history
        self.operation_history
            .push(LwwRegister::new("Cleared all entries".to_string()))?;

        // Trim history to last 100 entries (pop from front)
        while self.operation_history.len()? > 100 {
            // Vector doesn't have remove, so we'll just limit on read
            // For now, we'll keep all entries and limit in get_operation_history
            break;
        }

        Ok(())
    }

    /// Add a tag to the store
    pub fn add_tag(&mut self, tag: String) -> app::Result<()> {
        app::log!("Adding tag: {:?}", tag);
        self.tags.insert(tag)?;
        Ok(())
    }

    /// Remove a tag from the store
    pub fn remove_tag(&mut self, tag: &str) -> app::Result<bool> {
        app::log!("Removing tag: {:?}", tag);
        self.tags.remove(tag).map_err(Into::into)
    }

    /// Get all tags
    pub fn get_tags(&self) -> app::Result<Vec<String>> {
        Ok(self.tags.iter()?.collect())
    }

    /// Set store metadata
    pub fn set_metadata(&mut self, metadata: String) -> app::Result<()> {
        app::log!("Setting metadata: {:?}", metadata);
        self.metadata.set(metadata);
        Ok(())
    }

    /// Get store metadata
    pub fn get_metadata(&self) -> String {
        self.metadata.get().clone()
    }

    /// Get operation count
    pub fn get_operation_count(&self) -> app::Result<u64> {
        self.operation_count.value().map_err(Into::into)
    }

    /// Get operation history (last 100 entries)
    pub fn get_operation_history(&self) -> app::Result<Vec<String>> {
        let len = self.operation_history.len()?;
        let start = if len > 100 { len - 100 } else { 0 };
        let mut history = Vec::new();
        for i in start..len {
            if let Some(entry) = self.operation_history.get(i)? {
                history.push(entry.get().clone());
            }
        }
        Ok(history)
    }
}
