#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: UnorderedMap<String, LwwRegister<String>>,
    handlers_called: UnorderedMap<String, LwwRegister<String>>, // Track handlers called with CRDT counter
    handler_counter: Counter, // CRDT-compatible counter (stored separately like a collection)
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
            items: UnorderedMap::new(),
            handlers_called: UnorderedMap::new(),
            handler_counter: Counter::new(),
        }
    }
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.items.contains(&key)? {
            app::emit!((
                Event::Updated {
                    key: &key,
                    value: &value,
                },
                "update_handler"
            ));
        } else {
            app::emit!((
                Event::Inserted {
                    key: &key,
                    value: &value,
                },
                "insert_handler"
            ));
        }

        self.items.insert(key, value.into())?;

        Ok(())
    }

    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");

        Ok(self.items.entries()?.map(|(k, v)| (k, v.get().clone())).collect())
    }

    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");

        Ok(self.items.len()?)
    }

    pub fn get<'a>(&self, key: &'a str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn get_unchecked(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key without checking: {:?}", key);

        // this panics, which we do not recommend
        Ok(self.items.get(key)?.expect("key not found").get().clone())
    }

    pub fn get_result<'a>(&self, key: &'a str) -> app::Result<String> {
        app::log!("Getting key, possibly failing: {:?}", key);

        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };

        Ok(value)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);

        app::emit!((Event::Removed { key }, "remove_handler"));

        Ok(self.items.remove(key)?.map(|v| v.get().clone()))
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        app::emit!((Event::Cleared, "clear_handler"));

        self.items.clear().map_err(Into::into)
    }

    /// Helper function to log handler calls
    fn log_handler_call(&mut self, handler_name: &str, details: &str) -> app::Result<()> {
        app::log!("Handler '{}' called: {}", handler_name, details);

        // Increment CRDT counter (uses env::executor_id() internally)
        self.handler_counter.increment()?;

        Ok(())
    }

    /// Handle insert events
    pub fn insert_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
        self.log_handler_call("insert_handler", &format!("key={}, value={}", key, value))?;
        // Add your insert-specific logic here
        // For example: send notifications, update external systems, etc.
        Ok(())
    }

    /// Handle update events
    pub fn update_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
        self.log_handler_call("update_handler", &format!("key={}, value={}", key, value))?;
        // Add your update-specific logic here
        // For example: log changes, update caches, etc.
        Ok(())
    }

    /// Handle remove events
    pub fn remove_handler(&mut self, key: &str) -> app::Result<()> {
        self.log_handler_call("remove_handler", &format!("key={}", key))?;
        // Add your remove-specific logic here
        // For example: cleanup external resources, etc.
        Ok(())
    }

    /// Handle clear events
    pub fn clear_handler(&mut self) -> app::Result<()> {
        self.log_handler_call("clear_handler", "all items cleared")?;
        // Add your clear-specific logic here
        // For example: cleanup all external resources, etc.
        Ok(())
    }

    /// Get the total number of handlers executed across all nodes (CRDT G-Counter)
    ///
    /// This returns the global sum of handler executions from all nodes.
    /// The G-Counter ensures that concurrent increments from different nodes
    /// are correctly merged without conflicts.
    pub fn get_handler_execution_count(&self) -> app::Result<u64> {
        Ok(self.handler_counter.value()?)
    }

    /// Get the list of handlers that have been called (for testing)
    ///
    /// Note: This iterates through all possible handler keys from all nodes.
    /// The order may vary depending on CRDT merge timing.
    pub fn get_handlers_called(&self) -> app::Result<Vec<String>> {
        // Collect all handler entries from the map
        let mut handlers = Vec::new();
        for (key, value) in self.handlers_called.entries()? {
            if key.starts_with("handler_") {
                handlers.push(value.get().clone());
            }
        }
        handlers.sort();
        Ok(handlers)
    }

    /// Get unique handlers that have been called (for testing)
    /// This is more reliable than counting exact duplicates
    pub fn get_unique_handlers_called(&self) -> app::Result<Vec<String>> {
        let mut unique_handlers = std::collections::HashSet::new();
        for (key, value) in self.handlers_called.entries()? {
            if key.starts_with("handler_") {
                unique_handlers.insert(value.get().clone());
            }
        }
        let mut result: Vec<String> = unique_handlers.into_iter().collect();
        result.sort();
        Ok(result)
    }

    /// Check if a specific handler was called (for testing)
    pub fn was_handler_called(&self, handler_name: &str) -> app::Result<bool> {
        for (key, value) in self.handlers_called.entries()? {
            if key.starts_with("handler_") && value.get().contains(handler_name) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
