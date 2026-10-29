//! A string key-value store shared by every member of the app's organization: the
//! component SDK's basic example. State is one table of entries keyed by their key and a
//! counter of how many there are; every change emits an event to everyone the app reaches.
//!
//! Method names and arguments are the ABI's camelCase ones (`updateIfExists`,
//! `getOrInsert`, `getResult`, ..). The wasmer build of the same store, which the node's
//! own tests run, is `apps/wasmer-fixtures/kv-store`.

use calimero_app_sdk::prelude::*;

/// One stored value.
#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    /// The key it is stored under; any string, the empty string included.
    pub key: String,
    /// The value.
    pub value: String,
}

/// The store.
#[app::state(version = 1)]
pub struct KvStore {
    /// Every value by key. The leader applies one write at a time, so a later write to a
    /// key replaces an earlier one.
    entries: Table<Entry>,
    /// How many keys `entries` holds.
    size: Counter,
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

/// The most entries one `entries` call lists, and one `clear` pass removes.
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
        let event = if self.entries.contains(&key)? {
            Event::Updated {
                key: key.clone(),
                value: value.clone(),
            }
        } else {
            self.size.add(1)?;
            Event::Inserted {
                key: key.clone(),
                value: value.clone(),
            }
        };
        self.entries.upsert(Entry { key, value })?;
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// Replaces the value under `key` only if the key exists, and says whether it did:
    /// `false` writes nothing. Emits `Updated` when the value is replaced.
    pub fn update_if_exists(&mut self, key: String, value: String) -> app::Result<bool> {
        let updated = self.entries.update(&key, |e| e.value.clone_from(&value))?;
        if updated {
            env::emit(Event::Updated { key, value }, Audience::All)?;
        }
        Ok(updated)
    }

    /// The value under `key`, first storing `value` there if the key is absent (an
    /// existing value is returned unchanged). Emits `Inserted` only when it stores.
    pub fn get_or_insert(&mut self, key: String, value: String) -> app::Result<String> {
        if let Some(e) = self.entries.get(&key)? {
            return Ok(e.value);
        }
        self.entries.insert(Entry {
            key: key.clone(),
            value: value.clone(),
        })?;
        self.size.add(1)?;
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
    pub fn entries(&self) -> app::Result<std::collections::BTreeMap<String, String>> {
        let page = self.entries.scan(FindOpts::default().limit(PAGE))?;
        Ok(page.items.into_iter().map(|e| (e.key, e.value)).collect())
    }

    /// The number of keys; `"0"` when the store is empty.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.size.get()?)
    }

    /// The value under `key`, or `null` when it is absent.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?.map(|e| e.value))
    }

    /// The value under `key`; a missing key aborts the run (a trap, not an app error).
    /// Kept to show what a guest panic looks like: use `get` or `getResult`.
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        #[allow(clippy::expect_used, reason = "this method exists to show a panic")]
        let entry = self.entries.get(&key)?.expect("key not found");
        Ok(entry.value)
    }

    /// The value under `key`; `NotFound { key }` when it is absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.entries.get(&key)? {
            Some(e) => Ok(e.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key` and returns the value it held, or `null` when it was absent. Emits
    /// `Removed` only when a value was removed.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let Some(old) = self.entries.get(&key)? else {
            return Ok(None);
        };
        self.entries.remove(&key)?;
        self.size.add(-1)?;
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(Some(old.value))
    }

    /// Removes every key, as any member may (v1's `clear`, no confirmation). Emits
    /// `Cleared` only when the store was not already empty.
    pub fn clear(&mut self) -> app::Result<()> {
        let mut removed = false;
        loop {
            let page = self.entries.scan(FindOpts::default().limit(PAGE))?;
            if page.items.is_empty() {
                break;
            }
            for e in page.items {
                self.entries.remove(&e.key)?;
                self.size.add(-1)?;
                removed = true;
            }
        }
        if removed {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }
}
