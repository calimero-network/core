//! A key-value store whose `#[app::init]` seeds three entries: the init pattern.
//!
//! `init` runs once, as the app's owner, when the app is created; whatever it writes is the
//! app's first state, so every member's first read already sees the seeded keys.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct KvStoreInit {
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
            let _old = store.items.insert(&key.to_owned(), &value.to_owned())?;
        }
        env::log("KvStoreInit initialized with 3 default items");
        Ok(store)
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

    /// Every entry, in key order.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        let mut opts = FindOpts::default().limit(BATCH);
        loop {
            let page = self.items.page(opts.clone())?;
            out.extend(page.items);
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => return Ok(out),
            }
        }
    }

    /// How many keys there are.
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        Ok(self.items.len()?)
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.items.get(&key)?)
    }

    /// The value of `key`; panics (a trap, which we do not recommend) when absent.
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        Ok(self.items.get(&key)?.expect("key not found"))
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.items.get(&key)? {
            Some(value) => Ok(value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes `key`; returns the value it had. Emits `Removed` either way.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let old = self.items.remove(&key)?;
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(old)
    }

    /// Removes every entry; emits `Cleared`.
    pub fn clear(&mut self) -> app::Result<()> {
        let _removed = self.items.clear()?;
        env::emit(Event::Cleared, Audience::All)?;
        Ok(())
    }
}
