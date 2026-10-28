//! An element-ordered set: range queries, prefix scans, pagination, min and max.
//!
//! It is to a set what `sorted-kv-store` is to a map: a `Table` whose record is the element
//! alone, so the element is its own key, every read comes back in ascending element order,
//! and `range` / `prefix` / `page` / `first` / `last` read a slice of the set without loading
//! all of it. Think of it as a `BTreeSet`.

use calimero_app_sdk::prelude::*;

/// One element of the set.
#[derive(Record)]
#[table(key = "value")]
pub struct Element {
    pub value: String,
}

#[app::state(version = 1)]
pub struct SortedSetStore {
    items: Table<Element>,
    size: Counter,
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
        if self.items.contains(&value)? {
            return Ok(false);
        }
        self.items.insert(Element {
            value: value.clone(),
        })?;
        self.size.add(1)?;
        env::emit(Event::Added { value }, Audience::All)?;
        Ok(true)
    }

    /// Whether `value` is in the set.
    #[app::view]
    pub fn contains(&self, value: String) -> app::Result<bool> {
        Ok(self.items.contains(&value)?)
    }

    /// Removes `value`; `true` when it was present.
    pub fn remove(&mut self, value: String) -> app::Result<bool> {
        if !self.items.remove(&value)? {
            return Ok(false);
        }
        self.size.add(-1)?;
        env::emit(Event::Removed { value }, Audience::All)?;
        Ok(true)
    }

    /// Removes every element; `Cleared` only when there were any.
    pub fn clear(&mut self) -> app::Result<()> {
        let mut removed = false;
        loop {
            let page = self.items.scan(FindOpts::default().limit(BATCH))?;
            if page.items.is_empty() {
                break;
            }
            for e in page.items {
                self.items.remove(&e.value)?;
                self.size.add(-1)?;
                removed = true;
            }
        }
        if removed {
            env::emit(Event::Cleared, Audience::All)?;
        }
        Ok(())
    }

    /// The number of elements.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.size.get()?)
    }

    /// Whether the set has no elements.
    #[app::view]
    pub fn is_empty(&self) -> app::Result<bool> {
        Ok(self.size.get()? == 0)
    }

    /// All elements, ascending.
    #[app::view]
    pub fn elements(&self) -> app::Result<Vec<String>> {
        self.collect(FindOpts::default(), |_| true)
    }

    /// Elements within `[start, end)`, ascending: the table's own bounds, no full scan.
    #[app::view]
    pub fn range(&self, start: String, end: String) -> app::Result<Vec<String>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let opts = FindOpts::default()
            .from(vec![KeyPart::Str(start)])
            .to(vec![KeyPart::Str(end)]);
        self.collect(opts, |_| true)
    }

    /// Elements that start with `prefix`, ascending: the scan starts at `prefix` and stops at
    /// the first element past it.
    #[app::view]
    pub fn prefix(&self, prefix: String) -> app::Result<Vec<String>> {
        let opts = FindOpts::default().from(vec![KeyPart::Str(prefix.clone())]);
        self.collect(opts, |v| v.starts_with(&prefix))
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
            let page = self.items.scan(opts.clone())?;
            for e in page.items {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                out.push(e.value);
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
        let page = self.items.scan(FindOpts::default().limit(1))?;
        Ok(page.items.into_iter().next().map(|e| e.value))
    }

    /// The largest element.
    #[app::view]
    pub fn last(&self) -> app::Result<Option<String>> {
        let page = self.items.scan(FindOpts::default().limit(1).reverse())?;
        Ok(page.items.into_iter().next().map(|e| e.value))
    }

    /// The elements from `opts` on, while `keep` holds for them.
    fn collect(&self, opts: FindOpts, keep: impl Fn(&str) -> bool) -> app::Result<Vec<String>> {
        let mut out = Vec::new();
        let mut opts = opts.limit(BATCH);
        loop {
            let page = self.items.scan(opts.clone())?;
            for e in page.items {
                if !keep(&e.value) {
                    return Ok(out);
                }
                out.push(e.value);
            }
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => return Ok(out),
            }
        }
    }
}
