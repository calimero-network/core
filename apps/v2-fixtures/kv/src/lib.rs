//! The cluster tests' key-value fixture on the v2 SDK (v1's `apps/kv-store`, cut to what the
//! cluster tests call). Method names and arguments are the ABI's camelCase ones.

use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct Kv {
    entries: Table<Entry>,
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

/// The most entries one `entries` call lists.
const LIST_LIMIT: u32 = 1000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl Kv {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Sets `key`; emits `Inserted` for a new key, `Updated` for an existing one.
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        let event = self.upsert(&key, &value)?;
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// Sets `key` without an event (for values over the 16 KiB event limit).
    pub fn put(&mut self, key: String, value: String) -> app::Result<()> {
        let _event = self.upsert(&key, &value)?;
        Ok(())
    }

    /// Writes `n` keys (`fill-0` ..) of `size` bytes each, one `Inserted` event per new key.
    pub fn fill(&mut self, n: u32, size: u32) -> app::Result<()> {
        let value = "x".repeat(size as usize);
        for i in 0..n {
            let event = self.upsert(&format!("fill-{i}"), &value)?;
            env::emit(event, Audience::All)?;
        }
        Ok(())
    }

    /// Never returns: runs until the node's fuel or wall-clock budget stops it.
    pub fn spin(&mut self) -> app::Result<()> {
        let mut n: u64 = 0;
        loop {
            n = std::hint::black_box(n.wrapping_add(1));
        }
    }

    /// The entry timestamp this call ran at (D19); writes nothing.
    pub fn stamp(&mut self) -> app::Result<u64> {
        Ok(env::time_now())
    }

    /// The time a view sees: the last applied entry's timestamp (D19).
    #[app::view]
    pub fn time_now(&self) -> app::Result<u64> {
        Ok(env::time_now())
    }

    /// A fresh unique id (D52); writes nothing.
    pub fn new_id(&mut self) -> app::Result<String> {
        Ok(env::unique_id())
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?.map(|e| e.value))
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.entries.get(&key)? {
            Some(e) => Ok(e.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Every entry (up to 1000).
    #[app::view]
    pub fn entries(&self) -> app::Result<std::collections::BTreeMap<String, String>> {
        let page = self.entries.scan(FindOpts::default().limit(LIST_LIMIT))?;
        Ok(page.items.into_iter().map(|e| (e.key, e.value)).collect())
    }

    /// How many keys there are.
    #[app::view]
    pub fn len(&self) -> app::Result<u32> {
        Ok(u32::try_from(self.size.get()?).unwrap_or(u32::MAX))
    }

    /// Removes `key`; returns the value it had.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let Some(old) = self.entries.get(&key)? else {
            return Ok(None);
        };
        self.entries.remove(&key)?;
        self.size.add(-1)?;
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(Some(old.value))
    }

    /// Removes every entry. Needs a fresh passkey confirmation.
    #[app::confirm]
    pub fn clear(&mut self) -> app::Result<()> {
        self.remove_all()
    }

    /// The same as `clear`, under the brief's name.
    #[app::confirm]
    pub fn wipe(&mut self) -> app::Result<()> {
        self.remove_all()
    }

    fn upsert(&mut self, key: &str, value: &str) -> app::Result<Event> {
        let fresh = !self.entries.contains(key)?;
        self.entries.upsert(Entry {
            key: key.to_owned(),
            value: value.to_owned(),
        })?;
        if fresh {
            self.size.add(1)?;
            Ok(Event::Inserted {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        } else {
            Ok(Event::Updated {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        }
    }

    fn remove_all(&mut self) -> app::Result<()> {
        loop {
            let page = self.entries.scan(FindOpts::default().limit(LIST_LIMIT))?;
            if page.items.is_empty() {
                break;
            }
            for e in page.items {
                self.entries.remove(&e.key)?;
                self.size.add(-1)?;
            }
        }
        env::emit(Event::Cleared, Audience::All)?;
        Ok(())
    }
}
