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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { id } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(app) = env::state_read::<OnlyPeers>() else {
        env::panic_str("Failed to read app state.")
    };

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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input {} = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(app) = env::state_read::<OnlyPeers>() else {
        env::panic_str("Failed to read app state.")
    };

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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input { title, content } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let Some(mut app) = env::state_read::<OnlyPeers>() else {
        env::panic_str("Failed to read app state.")
    };

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

    let Some(input) = env::input() else {
        env::panic_str("Expected input since method has arguments.")
    };

    let Input {
        post_id,
        user,
        text,
    } = match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => env::panic_str(&format!("Failed to deserialize input from JSON: {:?}", err)),
    };

    let mut app = env::state_read::<OnlyPeers>().unwrap_or_default();

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
