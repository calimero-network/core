//! A key-ordered key-value store: range queries, prefix scans and pagination.
//!
//! Mirrors `kv-store` so the two can be compared side by side. The store is ordered by key,
//! so the map's reads come back in ascending key order, and `range` / `prefix` / `page` /
//! `first` / `last` read a slice of the keyspace without loading the whole store. Each
//! reads through the map's cursor, one page at a time.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;
use calimero_app_sdk::StorageError;

/// One key/value pair: what `first` and `last` return.
#[derive(serde::Serialize)]
pub struct Entry {
    pub key: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct SortedKvStore {
    items: UnorderedMap<String, String>,
}

#[app::error]
pub enum Error {
    NotFound { key: String },
}

#[app::event]
pub enum Event {
    Inserted { key: String, value: String },
    Updated { key: String, value: String },
    Removed { key: String },
    Cleared,
}

/// The page size the multi-page reads below fetch with.
const BATCH: u32 = 1000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl SortedKvStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Sets `key`; emits `Inserted` for a new key, `Updated` for an existing one.
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        let event = match self.items.insert(&key, &value)? {
            None => Event::Inserted { key, value },
            Some(_) => Event::Updated { key, value },
        };
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.items.get(&key)?)
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.items.get(&key)? {
            Some(value) => Ok(value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key` and returns its value. `Removed` only when a value was present: an
    /// absent key changes nothing, so nothing is broadcast.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let old = self.items.remove(&key)?;
        if old.is_some() {
            env::emit(Event::Removed { key }, Audience::All)?;
        }
        Ok(old)
    }

    /// Removes every entry; `Cleared` only when there were any.
    pub fn clear(&mut self) -> app::Result<()> {
        if self.items.clear()? > 0 {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }

    /// The number of entries.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.items.len()?)
    }

    /// Whether there are no entries.
    #[app::view]
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.items.is_empty()?)
    }

    /// All entries, in ascending key order.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        collect(|opts| self.items.page(opts))
    }

    /// Entries whose keys fall within `[start, end)`, ascending: the map's own bounds, no
    /// full scan.
    #[app::view]
    pub fn range(&self, start: String, end: String) -> app::Result<BTreeMap<String, String>> {
        if start >= end {
            return Ok(BTreeMap::new());
        }
        collect(|opts| self.items.range(&start, &end, opts))
    }

    /// Entries whose keys start with `prefix`, ascending, e.g. `prefix("user:")`: the read
    /// covers only the keys under it.
    #[app::view]
    pub fn prefix(&self, prefix: String) -> app::Result<BTreeMap<String, String>> {
        let prefix = [prefix.to_key_part()];
        collect(|opts| self.items.starts_with(&prefix, opts))
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
        let mut opts = FindOpts::default().limit(BATCH);
        loop {
            let page = self.items.page(opts.clone())?;
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

    /// The entry with the smallest key.
    #[app::view]
    pub fn first(&self) -> app::Result<Option<Entry>> {
        Ok(self.items.first()?.map(|(key, value)| Entry { key, value }))
    }

    /// The entry with the largest key.
    #[app::view]
    pub fn last(&self) -> app::Result<Option<Entry>> {
        Ok(self.items.last()?.map(|(key, value)| Entry { key, value }))
    }
}

/// Every page `read` returns, from the first on, in one map.
fn collect(
    read: impl Fn(FindOpts) -> Result<Page<(String, String)>, StorageError>,
) -> app::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut opts = FindOpts::default().limit(BATCH);
    loop {
        let page = read(opts.clone())?;
        out.extend(page.items);
        match page.next {
            Some(next) => opts = opts.cursor(next),
            None => return Ok(out),
        }
    }
}
