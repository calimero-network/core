//! A string key-value store shared by every member of the app's organization: the
//! component SDK's basic example. State is one ordered map of entries, which keeps its own
//! count; every change emits an event to everyone the app reaches.
//!
//! Method names and arguments are the ABI's camelCase ones (`updateIfExists`,
//! `getOrInsert`, `getResult`, ..). The wasmer build of the same store, which the node's
//! own tests run, is `apps/wasmer-fixtures/kv-store`.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;

/// The store.
#[app::state(version = 1)]
pub struct KvStore {
    /// Every value by key, with its count. The leader applies one write at a time, so a
    /// later write to a key replaces an earlier one.
    entries: UnorderedMap<String, String>,
}

/// A typed failure, `{code, fields}` on the wire.
#[app::error]
pub enum Error {
    /// No value is stored under `key`.
    NotFound { key: String },
}

/// A change to the store, emitted after the write that caused it.
#[app::event]
pub enum Event {
    /// A new key was stored.
    Inserted { key: String, value: String },
    /// An existing key's value was replaced.
    Updated { key: String, value: String },
    /// A key and its value were removed.
    Removed { key: String },
    /// Every key was removed from a non-empty store.
    Cleared,
}

/// The most entries one `entries` call lists.
const PAGE: u32 = 1000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl KvStore {
    /// Creates the empty store. The node runs it once, when the app is created.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Stores `value` under `key`, replacing any value already there. Emits `Inserted`
    /// for a new key and `Updated` for an existing one.
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        env::log(&format!("set {key:?} = {value:?}"));
        let event = match self.entries.insert(&key, &value)? {
            Some(_) => Event::Updated { key, value },
            None => Event::Inserted { key, value },
        };
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// Replaces the value under `key` only if the key exists, and says whether it did:
    /// `false` writes nothing. Emits `Updated` when the value is replaced.
    pub fn update_if_exists(&mut self, key: String, value: String) -> app::Result<bool> {
        if !self.entries.contains(&key)? {
            return Ok(false);
        }
        let _old = self.entries.insert(&key, &value)?;
        env::emit(Event::Updated { key, value }, Audience::All)?;
        Ok(true)
    }

    /// The value under `key`, first storing `value` there if the key is absent (an
    /// existing value is returned unchanged). Emits `Inserted` only when it stores.
    pub fn get_or_insert(&mut self, key: String, value: String) -> app::Result<String> {
        if let Some(existing) = self.entries.get(&key)? {
            return Ok(existing);
        }
        let _old = self.entries.insert(&key, &value)?;
        env::emit(
            Event::Inserted {
                key,
                value: value.clone(),
            },
            Audience::All,
        )?;
        Ok(value)
    }

    /// Every key with its value, as a JSON object ordered by key (up to 1000 keys);
    /// `{}` when the store is empty.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        let page = self.entries.page(FindOpts::default().limit(PAGE))?;
        Ok(page.items.into_iter().collect())
    }

    /// The number of keys; `"0"` when the store is empty.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.entries.len()?)
    }

    /// The value under `key`, or `null` when it is absent.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?)
    }

    /// The value under `key`; a missing key aborts the run (a trap, not an app error).
    /// Kept to show what a guest panic looks like: use `get` or `getResult`.
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        #[allow(clippy::expect_used, reason = "this method exists to show a panic")]
        let value = self.entries.get(&key)?.expect("key not found");
        Ok(value)
    }

    /// The value under `key`; `NotFound { key }` when it is absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.entries.get(&key)? {
            Some(value) => Ok(value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key` and returns the value it held, or `null` when it was absent. Emits
    /// `Removed` only when a value was removed.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let old = self.entries.remove(&key)?;
        if old.is_some() {
            env::emit(Event::Removed { key }, Audience::All)?;
        }
        Ok(old)
    }

    /// Removes every key, as any member may (v1's `clear`, no confirmation). Emits
    /// `Cleared` only when the store was not already empty.
    pub fn clear(&mut self) -> app::Result<()> {
        if self.entries.clear()? > 0 {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }
}
