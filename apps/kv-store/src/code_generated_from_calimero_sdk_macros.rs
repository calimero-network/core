use calimero_sdk::env;
use serde::Deserialize;

use super::KvStore;

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn set() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
        value: String,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { key, value }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let mut app: KvStore = env::state_read().unwrap_or_default();

    app.set(key, value);

    env::state_write(&app);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn entries() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {}

    let input = env::input().expect("Expected input since method has arguments.");

    let Input {}: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let app: KvStore = env::state_read().expect("Failed to read app state.");

    let value = app.entries();

    let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

    env::value_return(&output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn get() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { key }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let app: KvStore = env::state_read().expect("Failed to read app state.");

    let value = app.get(&key);

    let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

    env::value_return(&output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn get_unchecked() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { key }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let app: KvStore = env::state_read().expect("Failed to read app state.");

    let value = app.get_unchecked(&key);

    let output = serde_json::to_vec(&value).expect("Failed to serialize output to JSON.");

    env::value_return(&output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn remove() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input<'a> {
        key: &'a str,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { key }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let mut app: KvStore = env::state_read().unwrap_or_default();

    app.remove(key);

    env::state_write(&app);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn clear() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {}

    let input = env::input().expect("Expected input since method has arguments.");

    let Input {}: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let mut app: KvStore = env::state_read().unwrap_or_default();

    app.clear();

    env::state_write(&app);
}
