use std::collections::hash_map::{Entry as HashMapEntry, HashMap};

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::{app, env};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: HashMap<String, String>,
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
        KvStore::default()
    }

    pub fn set(&mut self, key: String, value: String) {
        env::log(&format!("Setting key: {:?} to value: {:?}", key, value));

        match self.items.entry(key) {
            HashMapEntry::Occupied(mut entry) => {
                app::emit!(Event::Updated {
                    key: entry.key(),
                    value: &value,
                });
                entry.insert(value);
            }
            HashMapEntry::Vacant(entry) => {
                app::emit!(Event::Inserted {
                    key: entry.key(),
                    value: &value,
                });
                entry.insert(value);
            }
        }
    }

    pub fn entries(&self) -> &HashMap<String, String> {
        env::log(&format!("Getting all entries"));

        &self.items
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        env::log(&format!("Getting key: {:?}", key));

        self.items.get(key).map(|v| v.as_str())
    }

    pub fn get_unchecked(&self, key: &str) -> &str {
        env::log(&format!("Getting key without checking: {:?}", key));

        match self.items.get(key) {
            Some(value) => value.as_str(),
            None => env::panic_str("Key not found."),
        }
    }

    pub fn get_result(&self, key: &str) -> Result<&str, &str> {
        env::log(&format!("Getting key, possibly failing: {:?}", key));

        self.get(key).ok_or("Key not found.")
    }

    pub fn remove(&mut self, key: &str) {
        env::log(&format!("Removing key: {:?}", key));

        app::emit!(Event::Removed { key });

        self.items.remove(key);
    }

    pub fn clear(&mut self) {
        env::log("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear();
    }
}
