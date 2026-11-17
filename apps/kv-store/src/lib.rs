//! A string key-value store shared by every member of the app's organization: the
//! component SDK's basic example. State is one ordered map of entries, which keeps its own
//! count; every change emits an event to everyone the app reaches.
//!
//! The map is ordered by key, so its reads come back in ascending key order, and `range` /
//! `prefix` / `page` / `first` / `last` read a slice of the keyspace without loading the
//! whole store: each reads through the map's cursor, one page at a time.
//!
//! Method names and arguments are the ABI's camelCase ones (`updateIfExists`,
//! `getOrInsert`, `getResult`, ..).

use std::collections::BTreeMap;

use calimero_sdk::prelude::*;
use calimero_sdk::StorageError;

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

/// One key and its value: what `first` and `last` return.
#[derive(serde::Serialize)]
pub struct Entry {
    pub key: String,
    pub value: String,
}

/// The most entries one `entries` call lists, and the page size the multi-page reads fetch with.
const PAGE: u32 = 1000;

#[app::logic]
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

    /// Whether the store holds no keys.
    #[app::view]
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.entries.is_empty()?)
    }

    /// Entries whose keys fall within `[start, end)`, ascending: the map's own bounds, no
    /// full scan. `{}` when `start` is not below `end`.
    #[app::view]
    pub fn range(&self, start: String, end: String) -> app::Result<BTreeMap<String, String>> {
        if start >= end {
            return Ok(BTreeMap::new());
        }
        collect(|opts| self.entries.range(&start, &end, opts))
    }

    /// Entries whose keys start with `prefix`, ascending, e.g. `prefix("user:")`: the read
    /// covers only the keys under it.
    #[app::view]
    pub fn prefix(&self, prefix: String) -> app::Result<BTreeMap<String, String>> {
        let prefix = [prefix.to_key_part()];
        collect(|opts| self.entries.starts_with(&prefix, opts))
    }

    /// `limit` entries after the first `offset`, ascending. The skipped entries are still
    /// read; a client paging a large store should prefer `range` from the last key it holds.
    #[app::view]
    pub fn page(&self, offset: u32, limit: u32) -> app::Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        if limit == 0 {
            return Ok(out);
        }
        let mut skipped = 0;
        let mut opts = FindOpts::default().limit(PAGE);
        loop {
            let page = self.entries.page(opts.clone())?;
            for (key, value) in page.items {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                let _ = out.insert(key, value);
                if out.len() == limit as usize {
                    return Ok(out);
                }
            }
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => return Ok(out),
            }
        }
    }

    /// The entry with the smallest key, or `null` when the store is empty.
    #[app::view]
    pub fn first(&self) -> app::Result<Option<Entry>> {
        Ok(self
            .entries
            .first()?
            .map(|(key, value)| Entry { key, value }))
    }

    /// The entry with the largest key, or `null` when the store is empty.
    #[app::view]
    pub fn last(&self) -> app::Result<Option<Entry>> {
        Ok(self
            .entries
            .last()?
            .map(|(key, value)| Entry { key, value }))
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

/// Every page `read` returns, from the first on, in one map.
fn collect(
    read: impl Fn(FindOpts) -> Result<Page<(String, String)>, StorageError>,
) -> app::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut opts = FindOpts::default().limit(PAGE);
    loop {
        let page = read(opts.clone())?;
        out.extend(page.items);
        match page.next {
            Some(next) => opts = opts.cursor(next),
            None => return Ok(out),
        }
    }
}
