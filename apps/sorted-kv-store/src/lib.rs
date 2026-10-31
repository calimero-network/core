//! A key-ordered key-value store: range queries, prefix scans and pagination.
//!
//! Mirrors `kv-store` so the two can be compared side by side. A `Table` keeps its records in
//! key order, so every read here comes back in ascending key order, and `range` / `prefix` /
//! `page` / `first` / `last` read a slice of the keyspace without loading the whole store.
//! Each reads through the table's cursor, one page at a time.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;

/// One key/value pair; also what `first` and `last` return.
#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct SortedKvStore {
    items: Table<Entry>,
    size: Counter,
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
        let fresh = !self.items.contains(&key)?;
        self.items.upsert(Entry {
            key: key.clone(),
            value: value.clone(),
        })?;
        let event = if fresh {
            self.size.add(1)?;
            Event::Inserted { key, value }
        } else {
            Event::Updated { key, value }
        };
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.items.get(&key)?.map(|e| e.value))
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.items.get(&key)? {
            Some(e) => Ok(e.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key` and returns its value. `Removed` only when a value was present: an
    /// absent key changes nothing, so nothing is broadcast.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let Some(old) = self.items.get(&key)? else {
            return Ok(None);
        };
        self.items.remove(&key)?;
        self.size.add(-1)?;
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(Some(old.value))
    }

    /// Removes every entry; `Cleared` only when there were any.
    pub fn clear(&mut self) -> app::Result<()> {
        let mut removed = false;
        loop {
            let page = self.items.scan(FindOpts::default().limit(BATCH))?;
            if page.items.is_empty() {
                break;
            }
            for e in page.items {
                self.items.remove(&e.key)?;
                self.size.add(-1)?;
                removed = true;
            }
        }
        if removed {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }

    /// The number of entries.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.size.get()?)
    }

    /// Whether there are no entries.
    #[app::view]
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.size.get()? == 0)
    }

    /// All entries, in ascending key order.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        self.collect(FindOpts::default(), |_| true)
    }

    /// Entries whose keys fall within `[start, end)`, ascending: the table's own bounds, no
    /// full scan.
    #[app::view]
    pub fn range(&self, start: String, end: String) -> app::Result<BTreeMap<String, String>> {
        if start >= end {
            return Ok(BTreeMap::new());
        }
        let opts = FindOpts::default()
            .from(vec![KeyPart::Str(start)])
            .to(vec![KeyPart::Str(end)]);
        self.collect(opts, |_| true)
    }

    /// Entries whose keys start with `prefix`, ascending, e.g. `prefix("user:")`: the scan
    /// starts at `prefix` and stops at the first key past it.
    #[app::view]
    pub fn prefix(&self, prefix: String) -> app::Result<BTreeMap<String, String>> {
        let opts = FindOpts::default().from(vec![KeyPart::Str(prefix.clone())]);
        self.collect(opts, |k| k.starts_with(&prefix))
    }

    /// `limit` entries after the first `offset`, ascending. The skipped entries are still
    /// read; a client paging a large store should prefer `range` from the last key it holds.
    #[app::view]
    pub fn page(&self, offset: u32, limit: u32) -> app::Result<BTreeMap<String, String>> {
        let mut skipped = 0;
        let mut out = BTreeMap::new();
        if limit == 0 {
            return Ok(out);
        }
        let mut opts = FindOpts::default().limit(BATCH);
        loop {
            let page = self.items.scan(opts.clone())?;
            for e in page.items {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                let _ = out.insert(e.key, e.value);
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
        let page = self.items.scan(FindOpts::default().limit(1))?;
        Ok(page.items.into_iter().next())
    }

    /// The entry with the largest key.
    #[app::view]
    pub fn last(&self) -> app::Result<Option<Entry>> {
        let page = self.items.scan(FindOpts::default().limit(1).reverse())?;
        Ok(page.items.into_iter().next())
    }

    /// The entries from `opts` on, while `keep` holds for their keys.
    fn collect(
        &self,
        opts: FindOpts,
        keep: impl Fn(&str) -> bool,
    ) -> app::Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        let mut opts = opts.limit(BATCH);
        loop {
            let page = self.items.scan(opts.clone())?;
            for e in page.items {
                if !keep(&e.key) {
                    return Ok(out);
                }
                let _ = out.insert(e.key, e.value);
            }
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => return Ok(out),
            }
        }
    }
}
