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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { key, value } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input {} = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(app) = env::state_read::<KvStore>() else {
        env::panic_str("Failed to read app state.")
    };

    let value = app.entries();

    let output = {
        #[allow(unused_imports)]
        use calimero_sdk::__private::IntoResult;
        match calimero_sdk::__private::WrappedReturn::new(value)
            .into_result()
            .to_json()
        {
            Ok(value) => value,
            Err(err) => env::panic_str(&format!("Failed to serialize output to JSON: {:?}", err)),
        }
    };

    env::value_return(output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn get() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
    }

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { key } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let app = match env::state_read::<KvStore>() {
        Some(value) => value,
        None => env::panic_str("Failed to read app state."),
    };

    let value = app.get(&key);

    let output = {
        #[allow(unused_imports)]
        use calimero_sdk::__private::IntoResult;
        match calimero_sdk::__private::WrappedReturn::new(value)
            .into_result()
            .to_json()
        {
            Ok(value) => value,
            Err(err) => env::panic_str(&format!("Failed to serialize output to JSON: {:?}", err)),
        }
    };

    env::value_return(output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn get_unchecked() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
    }

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { key } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(app) = env::state_read::<KvStore>() else {
        env::panic_str("Failed to read app state.")
    };

    let value = app.get_unchecked(&key);

    let output = {
        #[allow(unused_imports)]
        use calimero_sdk::__private::IntoResult;
        match calimero_sdk::__private::WrappedReturn::new(value)
            .into_result()
            .to_json()
        {
            Ok(value) => value,
            Err(err) => env::panic_str(&format!("Failed to serialize output to JSON: {:?}", err)),
        }
    };

    env::value_return(output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn get_result() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input {
        key: String,
    }

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { key } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(app) = env::state_read::<KvStore>() else {
        env::panic_str("Failed to read app state.")
    };

    let value = app.get_result(&key);

    let output = {
        #[allow(unused_imports)]
        use calimero_sdk::__private::IntoResult;
        match calimero_sdk::__private::WrappedReturn::new(value)
            .into_result()
            .to_json()
        {
            Ok(value) => value,
            Err(err) => env::panic_str(&format!("Failed to serialize output to JSON: {:?}", err)),
        }
    };

    env::value_return(output);
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn remove() {
    env::setup_panic_hook();

    #[derive(Deserialize)]
    struct Input<'a> {
        key: &'a str,
    }

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { key } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input {} = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let mut app: KvStore = env::state_read().unwrap_or_default();

    app.clear();

    env::state_write(&app);
}
