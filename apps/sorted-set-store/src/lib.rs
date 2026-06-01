//! Canonical example for [`SortedSet`] — an element-ordered collection that
//! supports range queries, prefix scans, pagination, and min/max (core#2559).
//!
//! It is to [`SortedSet`] what `sorted-kv-store` is to `SortedMap`: the same
//! add-wins union CRDT and on-disk ordered index, but the element doubles as its
//! own key, so iteration comes back in ascending element order and you can read a
//! slice of the set (`range` / `prefix` / `page` / `first` / `last`) without
//! loading all of it. Think of it as the `BTreeSet` to `UnorderedSet`'s
//! `HashSet`.

use calimero_sdk::app;
use calimero_storage::collections::SortedSet;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug)]
pub struct SortedSetStore {
    items: SortedSet<String>,
}

#[app::event]
pub enum Event<'a> {
    Added { value: &'a str },
    Removed { value: &'a str },
    Cleared,
}

#[app::logic]
impl SortedSetStore {
    #[app::init]
    pub fn init() -> SortedSetStore {
        SortedSetStore {
            items: SortedSet::new_with_field_name("items"),
        }
    }

    /// Add `value`; returns `true` if it was newly inserted (already-present
    /// elements are a no-op, as in any set).
    pub fn add(&mut self, value: String) -> app::Result<bool> {
        app::log!("Adding value: {:?}", value);

        let added = self.items.insert(value.clone())?;
        if added {
            app::emit!(Event::Added { value: &value });
        }

        Ok(added)
    }

    /// Whether `value` is in the set.
    pub fn contains(&self, value: &str) -> app::Result<bool> {
        app::log!("Checking membership: {:?}", value);

        Ok(self.items.contains(value)?)
    }

    /// Remove `value`; returns `true` if it was present.
    pub fn remove(&mut self, value: &str) -> app::Result<bool> {
        app::log!("Removing value: {:?}", value);

        let removed = self.items.remove(value)?;
        if removed {
            app::emit!(Event::Removed { value });
        }

        Ok(removed)
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all elements");

        // Mirror `add`/`remove`: only emit when the set actually changed, and
        // only after the clear succeeds.
        let was_non_empty = !self.items.is_empty()?;
        self.items.clear()?;
        if was_non_empty {
            app::emit!(Event::Cleared);
        }

        Ok(())
    }

    /// The number of elements in the set.
    pub fn len(&self) -> app::Result<usize> {
        Ok(self.items.len()?)
    }

    /// Whether the set has no elements.
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.items.is_empty()?)
    }

    /// All elements, **in ascending order** (the headline difference from an
    /// `UnorderedSet`).
    pub fn elements(&self) -> app::Result<Vec<String>> {
        app::log!("Getting all elements (sorted)");

        Ok(self.items.iter()?.collect())
    }

    /// Elements within `[start, end)`, ascending — a range query backed by the
    /// ordered index (no full scan).
    pub fn range(&self, start: String, end: String) -> app::Result<Vec<String>> {
        app::log!("Range query: [{:?}, {:?})", start, end);

        Ok(self.items.range(start..end)?.collect())
    }

    /// Elements that start with `prefix`, ascending — e.g. `prefix("user:")`.
    pub fn prefix(&self, prefix: String) -> app::Result<Vec<String>> {
        app::log!("Prefix scan: {:?}", prefix);

        Ok(self.items.prefix(prefix.as_bytes())?.collect())
    }

    /// A page of `limit` elements starting at `offset`, ascending — paginate
    /// without materialising the whole set. Returns an empty `Vec` once
    /// `offset` reaches or passes the end of the set.
    pub fn page(&self, offset: usize, limit: usize) -> app::Result<Vec<String>> {
        app::log!("Page: offset={offset} limit={limit}");

        Ok(self.items.page(offset, limit)?)
    }

    /// The smallest element.
    pub fn first(&self) -> app::Result<Option<String>> {
        self.items.first().map_err(Into::into)
    }

    /// The largest element.
    pub fn last(&self) -> app::Result<Option<String>> {
        self.items.last().map_err(Into::into)
    }
}
