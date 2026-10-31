//! A key-value store whose `#[app::init]` seeds three entries: the init pattern.
//!
//! `init` runs once, as the app's owner, when the app is created; whatever it writes is the
//! app's first state, so every member's first read already sees the seeded keys.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "key")]
pub struct Item {
    pub key: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct KvStoreInit {
    items: Table<Item>,
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

/// What `init` writes.
const SEED: [(&str, &str); 3] = [
    ("init_key_1", "Initial value 1"),
    ("init_key_2", "Initial value 2"),
    ("welcome", "Welcome to KvStoreInit!"),
];

/// The page size the multi-page reads below fetch with.
const BATCH: u32 = 1000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl KvStoreInit {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        env::log("Initializing KvStoreInit with default items");
        let mut store = Self::load();
        for (key, value) in SEED {
            let _fresh = store.upsert(key, value)?;
        }
        env::log("KvStoreInit initialized with 3 default items");
        Ok(store)
    }

    /// Sets `key`; emits `Inserted` for a new key, `Updated` for an existing one.
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        let event = if self.upsert(&key, &value)? {
            Event::Inserted { key, value }
        } else {
            Event::Updated { key, value }
        };
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// Every entry, in key order.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        let mut opts = FindOpts::default().limit(BATCH);
        loop {
            let page = self.items.scan(opts.clone())?;
            out.extend(page.items.into_iter().map(|i| (i.key, i.value)));
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => return Ok(out),
            }
        }
    }

    /// How many keys there are.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.size.get()?)
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.items.get(&key)?.map(|i| i.value))
    }

    /// The value of `key`; panics (a trap, which we do not recommend) when absent.
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        Ok(self.items.get(&key)?.expect("key not found").value)
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.items.get(&key)? {
            Some(i) => Ok(i.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key`; returns the value it had. Emits `Removed` either way.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let old = self.items.get(&key)?;
        if old.is_some() {
            self.items.remove(&key)?;
            self.size.add(-1)?;
        }
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(old.map(|i| i.value))
    }

    /// Removes every entry; emits `Cleared`.
    pub fn clear(&mut self) -> app::Result<()> {
        loop {
            let page = self.items.scan(FindOpts::default().limit(BATCH))?;
            if page.items.is_empty() {
                break;
            }
            for i in page.items {
                self.items.remove(&i.key)?;
                self.size.add(-1)?;
            }
        }
        env::emit(Event::Cleared, Audience::All)?;
        Ok(())
    }

    /// Writes `key`; `true` when it was new.
    fn upsert(&mut self, key: &str, value: &str) -> app::Result<bool> {
        let fresh = !self.items.contains(key)?;
        self.items.upsert(Item {
            key: key.to_owned(),
            value: value.to_owned(),
        })?;
        if fresh {
            self.size.add(1)?;
        }
        Ok(fresh)
    }
}
