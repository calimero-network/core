use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;

mod code_generated_from_calimero_sdk_macros;

#[derive(Default, BorshSerialize, BorshDeserialize)]
struct KvStore {
    items: HashMap<String, String>,
}

impl KvStore {
    fn set(&mut self, key: String, value: String) {
        env::log(&format!("Setting key: {:?} to value: {:?}", key, value));

        self.items.insert(key, value);
    }

    fn entries(&self) -> &HashMap<String, String> {
        env::log(&format!("Getting all entries"));

        &self.items
    }

    fn get(&self, key: &str) -> Option<&str> {
        env::log(&format!("Getting key: {:?}", key));

        self.items.get(key).map(|v| v.as_str())
    }

    fn get_unchecked(&self, key: &str) -> &str {
        env::log(&format!("Getting key without checking: {:?}", key));

        self.items.get(key).expect("Key not found.").as_str()
    }

    fn remove(&mut self, key: &str) {
        env::log(&format!("Removing key: {:?}", key));

        self.items.remove(key);
    }

    fn clear(&mut self) {
        env::log("Clearing all entries");

        self.items.clear();
    }
}
