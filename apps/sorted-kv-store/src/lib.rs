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
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, SortedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
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
            items: SortedMap::new_with_field_name("items"),
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

        app::emit!(Event::Removed { key });

        Ok(self.items.remove(key)?.map(|v| v.get().clone()))
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear().map_err(Into::into)
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
