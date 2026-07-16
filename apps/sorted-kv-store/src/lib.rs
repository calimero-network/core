//! Canonical example for [`SortedMap`] — a key-ordered collection that supports
//! range queries, prefix scans, and pagination (core#2559).
//!
//! Mirrors `kv-store` (the `UnorderedMap` example) so the two can be compared
//! side by side: the storage/CRDT behaviour is identical; the difference is that
//! every iteration here comes back in ascending key order, and the extra
//! `range` / `prefix` / `page` / `first` / `last` methods let you read a slice
//! of the keyspace without loading the whole map.

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, SortedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct SortedKvStore {
    items: SortedMap<String, LwwRegister<String>>,
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

/// A single key/value pair, returned by `first`/`last`. (A record rather than a
/// tuple because the ABI represents named-field records, not tuples.)
#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

#[app::logic]
impl SortedKvStore {
    #[app::init]
    pub fn init() -> SortedKvStore {
        SortedKvStore {
            items: SortedMap::new(),
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

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn get_result(&self, key: &str) -> app::Result<String> {
        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };

        Ok(value)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);

        // Only emit `Removed` when a value was actually present — emitting for
        // an absent key would broadcast a change that never happened.
        let removed = self.items.remove(key)?.map(|v| v.get().clone());
        if removed.is_some() {
            app::emit!(Event::Removed { key });
        }

        Ok(removed)
    }

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

    /// The number of entries in the map.
    pub fn len(&self) -> app::Result<usize> {
        Ok(self.items.len()?)
    }

    /// Whether the map has no entries.
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.items.is_empty()?)
    }

    /// All entries, **in ascending key order** (the headline difference from
    /// `kv-store`'s `UnorderedMap`).
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries (sorted)");

        Ok(self
            .items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Entries whose keys fall within `[start, end)`, ascending — a range query
    /// backed by the ordered index (no full scan). Returned as a key-ordered
    /// map.
    pub fn range(&self, start: String, end: String) -> app::Result<BTreeMap<String, String>> {
        app::log!("Range query: [{:?}, {:?})", start, end);

        Ok(self
            .items
            .range(start..end)?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Entries whose keys start with `prefix`, ascending — e.g. `prefix("user:")`.
    pub fn prefix(&self, prefix: String) -> app::Result<BTreeMap<String, String>> {
        app::log!("Prefix scan: {:?}", prefix);

        Ok(self
            .items
            .prefix(prefix.as_bytes())?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// A page of `limit` entries starting at `offset`, ascending — paginate
    /// without materialising the whole map.
    pub fn page(&self, offset: usize, limit: usize) -> app::Result<BTreeMap<String, String>> {
        app::log!("Page: offset={offset} limit={limit}");

        Ok(self
            .items
            .page(offset, limit)?
            .into_iter()
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// The entry with the smallest key.
    pub fn first(&self) -> app::Result<Option<Entry>> {
        Ok(self.items.first()?.map(|(key, v)| Entry {
            key,
            value: v.get().clone(),
        }))
    }

    /// The entry with the largest key.
    pub fn last(&self) -> app::Result<Option<Entry>> {
        Ok(self.items.last()?.map(|(key, v)| Entry {
            key,
            value: v.get().clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn set_get_and_ordered_entries() {
        let mut app = TestHost::new(SortedKvStore::init);

        app.call(|s| s.set("b".into(), "2".into())).unwrap();
        app.call(|s| s.set("a".into(), "1".into())).unwrap();

        assert_eq!(app.view(|s| s.get("a")).unwrap(), Some("1".to_owned()));

        // BTreeMap collects in ascending key order — the headline guarantee.
        let entries = app.view(|s| s.entries()).unwrap();
        let keys: Vec<_> = entries.keys().cloned().collect();
        assert_eq!(keys, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn remove_absent_and_clear_empty_emit_nothing() {
        let mut app = TestHost::new(SortedKvStore::init);

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
