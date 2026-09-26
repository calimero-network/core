#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::unordered_map::Entry;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
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
    /// Creates the empty store. The node runs it once, when the context is created.
    ///
    /// # Errors
    ///
    /// Fails with `Cannot initialize over already existing state.` when the
    /// context already has a store.
    ///
    /// # Examples
    ///
    /// ```json
    /// {}
    /// ```
    #[app::init]
    pub fn init() -> KvStore {
        KvStore {
            items: UnorderedMap::new(),
        }
    }

    /// Stores `value` under `key`, replacing any value already there.
    ///
    /// Emits `Inserted` for a new key and `Updated` for an existing one.
    /// Concurrent writes to the same key from different members converge to the
    /// latest write.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to write. Any string, including the empty string.
    /// * `value` - The value to store.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting", "value": "hello"}
    /// ```
    #[app::destructive]
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

    /// Replaces the value under `key` only if the key already exists, and returns
    /// whether it did.
    ///
    /// Returns `false` and writes nothing when `key` is absent. Emits `Updated`
    /// when the value is replaced.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to update. It must already exist for anything to change.
    /// * `value` - The new value.
    ///
    /// # Returns
    ///
    /// `true` when `key` existed and now holds `value`; `false` when `key` was
    /// absent, in which case nothing was written.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting", "value": "hi"}
    /// ```
    #[app::destructive]
    pub fn update_if_exists(&mut self, key: String, value: String) -> app::Result<bool> {
        // Demonstrates `get_mut`: the guard persists the change, with a new
        // timestamp, when it is dropped.
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

    /// Returns the value under `key`, first storing `value` there if the key is
    /// absent.
    ///
    /// An existing value is returned unchanged and `value` is ignored. Emits
    /// `Inserted` only when the key was absent.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to read, or to create when absent.
    /// * `value` - The value to store when `key` is absent.
    ///
    /// # Returns
    ///
    /// The value `key` holds after the call: the existing value when `key` was
    /// present, otherwise `value`.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting", "value": "hello"}
    /// ```
    #[app::idempotent]
    pub fn get_or_insert(&mut self, key: String, value: String) -> app::Result<String> {
        // Demonstrates the `entry` API combined with `or_insert`.
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

    /// Returns every key with its value, as a JSON object ordered by key.
    ///
    /// # Returns
    ///
    /// A JSON object mapping every key to its value, ordered by key; `{}` when the
    /// store is empty.
    ///
    /// # Examples
    ///
    /// ```json
    /// {}
    /// ```
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");

        Ok(self
            .items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Returns the number of keys in the store.
    ///
    /// # Returns
    ///
    /// The number of keys in the store; `0` when it is empty.
    ///
    /// # Examples
    ///
    /// ```json
    /// {}
    /// ```
    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");

        Ok(self.items.len()?)
    }

    /// Returns the value under `key`, or `null` when the key is absent.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to read.
    ///
    /// # Returns
    ///
    /// The value stored under `key`, or `null` when `key` is absent.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting"}
    /// ```
    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    /// Returns the value under `key`, aborting the call when the key is absent.
    ///
    /// Kept to show what a guest panic looks like; use `get` or `get_result`.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to read. It must exist.
    ///
    /// # Returns
    ///
    /// The value stored under `key`. An absent key yields no value: the call aborts.
    ///
    /// # Errors
    ///
    /// Fails with a guest panic (`key not found`) when `key` is absent.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting"}
    /// ```
    pub fn get_unchecked(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key without checking: {:?}", key);

        // this panics, which we do not recommend
        Ok(self.items.get(key)?.expect("key not found").get().clone())
    }

    /// Returns the value under `key`, failing with a typed error when it is absent.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to read.
    ///
    /// # Returns
    ///
    /// The value stored under `key`. An absent key yields no value: the call fails
    /// with `NotFound`.
    ///
    /// # Errors
    ///
    /// Fails with `{"kind": "NotFound", "data": "<key>"}` when `key` is absent.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting"}
    /// ```
    pub fn get_result(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key, possibly failing: {:?}", key);

        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };

        Ok(value)
    }

    /// Removes `key` and returns the value it held, or `null` when it was absent.
    ///
    /// Emits `Removed` only when a value was actually removed.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to remove.
    ///
    /// # Returns
    ///
    /// The value `key` held before the call, or `null` when `key` was absent.
    ///
    /// # Examples
    ///
    /// ```json
    /// {"key": "greeting"}
    /// ```
    #[app::destructive]
    #[app::idempotent]
    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);

        // Only emit `Removed` when a value was actually present - emitting for
        // an absent key would broadcast a change that never happened.
        let removed = self.items.remove(key)?.map(|v| v.get().clone());
        if removed.is_some() {
            app::emit!(Event::Removed { key });
        }

        Ok(removed)
    }

    /// Removes every key.
    ///
    /// Emits `Cleared` only when the store was not already empty.
    ///
    /// # Examples
    ///
    /// ```json
    /// {}
    /// ```
    #[app::destructive]
    #[app::idempotent]
    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        // Only emit `Cleared` when the map had entries, and only after the
        // clear succeeds.
        let was_non_empty = !self.items.is_empty()?;
        self.items.clear()?;
        if was_non_empty {
            app::emit!(Event::Cleared);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn set_get_len_remove() {
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.set("k".into(), "v".into())).unwrap();
        assert_eq!(app.view(|s| s.get("k")).unwrap(), Some("v".to_owned()));
        assert_eq!(app.view(|s| s.len()).unwrap(), 1);

        assert_eq!(app.call(|s| s.remove("k")).unwrap(), Some("v".to_owned()));
        assert_eq!(app.view(|s| s.get("k")).unwrap(), None);
        assert_eq!(app.view(|s| s.len()).unwrap(), 0);
    }

    #[test]
    fn entries_and_clear() {
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.set("a".into(), "1".into())).unwrap();
        app.call(|s| s.set("b".into(), "2".into())).unwrap();

        let entries = app.view(|s| s.entries()).unwrap();
        assert_eq!(entries.get("a"), Some(&"1".to_owned()));
        assert_eq!(entries.get("b"), Some(&"2".to_owned()));

        app.call(|s| s.clear()).unwrap();
        assert_eq!(app.view(|s| s.len()).unwrap(), 0);
    }

    #[test]
    fn update_if_exists_and_get_or_insert() {
        let mut app = TestHost::new(KvStore::init);

        // Nothing to update yet.
        assert!(!app
            .call(|s| s.update_if_exists("k".into(), "v".into()))
            .unwrap());

        // Inserts on first call, returns the existing value afterwards.
        assert_eq!(
            app.call(|s| s.get_or_insert("k".into(), "first".into()))
                .unwrap(),
            "first".to_owned()
        );
        assert_eq!(
            app.call(|s| s.get_or_insert("k".into(), "second".into()))
                .unwrap(),
            "first".to_owned()
        );

        // Now the key exists, so the update lands.
        assert!(app
            .call(|s| s.update_if_exists("k".into(), "v".into()))
            .unwrap());
        assert_eq!(app.view(|s| s.get("k")).unwrap(), Some("v".to_owned()));
    }

    #[test]
    fn set_emits_event() {
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.set("k".into(), "v".into())).unwrap();
        assert_eq!(app.events().len(), 1);
    }

    #[test]
    fn remove_absent_and_clear_empty_emit_nothing() {
        let mut app = TestHost::new(KvStore::init);

        // Removing a key that was never set must not broadcast a change.
        assert_eq!(app.call(|s| s.remove("missing")).unwrap(), None);
        assert!(app.events().is_empty());

        // Clearing an already-empty store likewise emits nothing.
        app.call(|s| s.clear()).unwrap();
        assert!(app.events().is_empty());

        // A real removal still emits exactly one `Removed` event.
        app.call(|s| s.set("k".into(), "v".into())).unwrap();
        let _ = app.take_events();
        assert_eq!(app.call(|s| s.remove("k")).unwrap(), Some("v".to_owned()));
        let events = app.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Removed");

        // And a real clear (non-empty store) emits exactly one `Cleared`.
        app.call(|s| s.set("k".into(), "v".into())).unwrap();
        let _ = app.take_events();
        app.call(|s| s.clear()).unwrap();
        let events = app.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Cleared");
    }
}
