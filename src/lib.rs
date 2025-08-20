#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::UnorderedMap;
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: UnorderedMap<String, String>,
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
        app::log!("Initializing TypeScript-compatible KV Store");
        KvStore {
            items: UnorderedMap::new(),
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

        self.items.insert(key, value)?;

        Ok(())
    }

    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");
        Ok(self.items.entries()?.collect())
    }

    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");
        Ok(self.items.len()?)
    }

    pub fn get<'a>(&self, key: &'a str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);
        self.items.get(key).map_err(Into::into)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);
        app::emit!(Event::Removed { key });
        self.items.remove(key).map_err(Into::into)
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");
        app::emit!(Event::Cleared);
        self.items.clear().map_err(Into::into)
    }

    pub fn run_demo(&mut self) -> app::Result<()> {
        app::log!("Running TypeScript-compatible KV Store Demo");
        
        // Demo operations
        self.set("demo_key".to_string(), "demo_value".to_string())?;
        self.set("hello".to_string(), "world".to_string())?;
        self.set("typescript".to_string(), "rocks".to_string())?;
        
        app::log!("Store now has {} entries", self.len()?);
        
        // Test get operations
        if let Some(value) = self.get("hello")? {
            app::log!("Retrieved 'hello' = {}", value);
        }
        
        // Test remove operation
        self.remove("demo_key")?;
        app::log!("After removal, store has {} entries", self.len()?);
        
        app::log!("Demo completed successfully");
        Ok(())
    }
}
