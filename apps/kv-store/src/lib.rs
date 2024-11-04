#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::types::Error;
use calimero_sdk::{app, env};
use calimero_storage::collections::UnorderedMap;
use calimero_storage::entities::Element;
use calimero_storage::AtomicUnit;

mod __private;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[root]
#[type_id(1)]
pub struct KvStore {
    items: UnorderedMap<String, String>,
    #[storage]
    storage: Element,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        KvStore {
            items: UnorderedMap::new().unwrap(),
            storage: Element::root(),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> Result<(), Error> {
        env::log(&format!("Setting key: {:?} to value: {:?}", key, value));

        if self.items.get(&key)?.is_some() {
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

        self.items.insert(key, value)?;

        Ok(())
    }

    pub fn entries(&self) -> Result<BTreeMap<String, String>, Error> {
        env::log("Getting all entries");

        Ok(self.items.entries()?.collect())
    }

    pub fn len(&self) -> Result<usize, Error> {
        env::log("Getting the number of entries");

        Ok(self.items.len()?)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, Error> {
        env::log(&format!("Getting key: {:?}", key));

        self.items.get(key).map_err(Into::into)
    }

    pub fn get_unchecked(&self, key: &str) -> Result<String, Error> {
        env::log(&format!("Getting key without checking: {:?}", key));

        Ok(self.items.get(key)?.expect("Key not found."))
    }

    pub fn get_result(&self, key: &str) -> Result<String, Error> {
        env::log(&format!("Getting key, possibly failing: {:?}", key));

        self.get(key)?.ok_or_else(|| Error::msg("Key not found."))
    }

    pub fn remove(&mut self, key: &str) -> Result<bool, Error> {
        env::log(&format!("Removing key: {:?}", key));

        app::emit!(Event::Removed { key });

        self.items.remove(key).map_err(Into::into)
    }

    pub fn clear(&mut self) -> Result<(), Error> {
        env::log("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear().map_err(Into::into)
    }
}
