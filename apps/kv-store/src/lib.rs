use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;

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
}

mod code_generated_from_calimero_sdk_macros {
    use calimero_sdk::env;
    use serde::{Deserialize, Serialize};

    use super::KvStore;

    #[no_mangle]
    pub extern "C" fn set() {
        env::setup_panic_hook();

        #[derive(Serialize, Deserialize)]
        struct Input {
            key: String,
            value: String,
        }

        let Input { key, value }: Input = serde_json::from_slice(
            &env::input().expect("Expected input since method has arguments."),
        )
        .expect("Failed to deserialize input from JSON.");

        let mut app: KvStore = env::state_read().unwrap_or_default();

        app.set(key, value);

        env::state_write(&app);
    }

    #[no_mangle]
    pub extern "C" fn entries() {
        env::setup_panic_hook();

        #[derive(Serialize, Deserialize)]
        struct Input {}

        let Input {}: Input = serde_json::from_slice(
            &env::input().expect("Expected input since method has arguments."),
        )
        .expect("Failed to deserialize input from JSON.");

        let app: KvStore = env::state_read().expect("Failed to read app state.");

        let value = app.entries();

        let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

        env::value_return(&output);
    }

    #[no_mangle]
    pub extern "C" fn get() {
        env::setup_panic_hook();

        #[derive(Serialize, Deserialize)]
        struct Input {
            key: String,
        }

        let Input { key }: Input = serde_json::from_slice(
            &env::input().expect("Expected input since method has arguments."),
        )
        .expect("Failed to deserialize input from JSON.");

        let app: KvStore = env::state_read().expect("Failed to read app state.");

        let value = app.get(&key);

        let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

        env::value_return(&output);
    }

    #[no_mangle]
    pub extern "C" fn get_unchecked() {
        env::setup_panic_hook();

        #[derive(Serialize, Deserialize)]
        struct Input {
            key: String,
        }

        let Input { key }: Input = serde_json::from_slice(
            &env::input().expect("Expected input since method has arguments."),
        )
        .expect("Failed to deserialize input from JSON.");

        let app: KvStore = env::state_read().expect("Failed to read app state.");

        let value = app.get_unchecked(&key);

        let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

        env::value_return(&output);
    }
}
