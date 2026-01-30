#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::unordered_map::Entry;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: UnorderedMap<String, LwwRegister<String>>,
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
        KvStore {
            // Use deterministic ID based on field name for sync compatibility
            items: UnorderedMap::new_with_field_name("items"),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.items.contains(&key)? {
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

        self.items.insert(key, value.into())?;

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

        Ok(self.items.remove(key)?.map(|v| v.get().clone()))
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear().map_err(Into::into)
    }
}
