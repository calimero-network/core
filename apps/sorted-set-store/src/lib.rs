//! An element-ordered set: range queries, prefix scans, pagination, min and max.
//!
//! It is to a set what `sorted-kv-store` is to a map: a `Set` keeps its elements in order
//! and its own count, so every read comes back in ascending element order, and `range` /
//! `prefix` / `page` / `first` / `last` read a slice of the set without loading all of it.
//! Think of it as a `BTreeSet`.

use calimero_app_sdk::prelude::*;
use calimero_app_sdk::StorageError;

#[app::state(version = 1)]
pub struct SortedSetStore {
    items: Set<String>,
}

#[app::event]
pub enum Event {
    Added { value: String },
    Removed { value: String },
    Cleared,
}

/// The page size the multi-page reads below fetch with.
const BATCH: u32 = 1000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl SortedSetStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Adds `value`; `true` when it was new (an element already present is a no-op).
    pub fn add(&mut self, value: String) -> app::Result<bool> {
        let added = self.items.insert(&value)?;
        if added {
            env::emit(Event::Added { value }, Audience::All)?;
        }
        Ok(added)
    }

    /// Whether `value` is in the set.
    #[app::view]
    pub fn contains(&self, value: String) -> app::Result<bool> {
        Ok(self.items.contains(&value)?)
    }

    /// Removes `value`; `true` when it was present.
    pub fn remove(&mut self, value: String) -> app::Result<bool> {
        let removed = self.items.remove(&value)?;
        if removed {
            env::emit(Event::Removed { value }, Audience::All)?;
        }
        Ok(removed)
    }

    /// Removes every element; `Cleared` only when there were any.
    pub fn clear(&mut self) -> app::Result<()> {
        if self.items.clear()? > 0 {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }

    /// The number of elements.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.items.len()?)
    }

    /// Whether the set has no elements.
    #[app::view]
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.items.is_empty()?)
    }

    /// All elements, ascending.
    #[app::view]
    pub fn elements(&self) -> app::Result<Vec<String>> {
        collect(|opts| self.items.page(opts))
    }

    /// Elements within `[start, end)`, ascending: the set's own bounds, no full scan.
    #[app::view]
    pub fn range(&self, start: String, end: String) -> app::Result<Vec<String>> {
        if start >= end {
            return Ok(Vec::new());
        }
        collect(|opts| self.items.range(&start, &end, opts))
    }

    /// Elements that start with `prefix`, ascending: the read covers only the elements
    /// under it.
    #[app::view]
    pub fn prefix(&self, prefix: String) -> app::Result<Vec<String>> {
        let prefix = [prefix.to_key_part()];
        collect(|opts| self.items.starts_with(&prefix, opts))
    }

    /// `limit` elements after the first `offset`, ascending; empty once `offset` reaches the
    /// end. The skipped elements are still read; to page a large set, prefer `range` from the
    /// last element held.
    #[app::view]
    pub fn page(&self, offset: u32, limit: u32) -> app::Result<Vec<String>> {
        let mut out = Vec::new();
        if limit == 0 {
            return Ok(out);
        }
        let mut skipped = 0;
        let mut opts = FindOpts::default().limit(BATCH);
        loop {
            let page = self.items.page(opts.clone())?;
            for value in page.items {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                out.push(value);
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

    /// The smallest element.
    #[app::view]
    pub fn first(&self) -> app::Result<Option<String>> {
        Ok(self.items.first()?)
    }

    /// The largest element.
    #[app::view]
    pub fn last(&self) -> app::Result<Option<String>> {
        Ok(self.items.last()?)
    }
}

/// Every page `read` returns, from the first on, in order.
fn collect(
    read: impl Fn(FindOpts) -> Result<Page<String>, StorageError>,
) -> app::Result<Vec<String>> {
    let mut out = Vec::new();
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
