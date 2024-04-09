use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::{app, env};

#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
struct KvStore {
    items: HashMap<String, String>,
}

#[app::logic]
impl KvStore {
    pub fn set(&mut self, key: &str, value: &str) {
        env::log(&format!("Setting key: {:?} to value: {:?}", key, value));

        self.items.insert(key.to_owned(), value.to_owned());
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

        self.items.remove(key);
    }

    pub fn clear(&mut self) {
        env::log("Clearing all entries");

        self.items.clear();
    }
}
