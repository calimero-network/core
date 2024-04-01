use calimero_sdk::env;
use serde::{Deserialize, Serialize};

use super::OnlyPeers;

#[no_mangle]
pub extern "C" fn post() {
    env::setup_panic_hook();

    #[derive(Serialize, Deserialize)]
    struct Input {
        id: usize,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { id }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let app: OnlyPeers = env::state_read().expect("Failed to read app state.");

    let value = app.post(id);

    let output = {
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

#[no_mangle]
pub extern "C" fn posts() {
    env::setup_panic_hook();

    #[derive(Serialize, Deserialize)]
    struct Input {}

    let input = env::input().expect("Expected input since method has arguments.");

    let Input {}: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let app: OnlyPeers = env::state_read().expect("Failed to read app state.");

    let value = app.posts();

    let output = {
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

#[no_mangle]
pub extern "C" fn create_post() {
    env::setup_panic_hook();

    #[derive(Serialize, Deserialize)]
    struct Input {
        title: String,
        content: String,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input { title, content }: Input =
        serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let mut app: OnlyPeers = env::state_read().unwrap_or_default();

    let value = app.create_post(title, content);

    let output = {
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

    env::state_write(&app);
}

#[no_mangle]
pub extern "C" fn create_comment() {
    env::setup_panic_hook();

    #[derive(Serialize, Deserialize)]
    struct Input {
        post_id: usize,
        user: String,
        text: String,
    }

    let input = env::input().expect("Expected input since method has arguments.");

    let Input {
        post_id,
        user,
        text,
    }: Input = serde_json::from_slice(&input).expect("Failed to deserialize input from JSON.");

    let mut app: OnlyPeers = env::state_read().unwrap_or_default();

    let value = app.create_comment(post_id, user, text);

    let output = {
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

    env::state_write(&app);
}
