use rand::Rng;
use rand::thread_rng;

use reqwest::blocking::Client;
use serde::Serialize;

#[derive(Serialize)]
struct JsonRequestSend {
    jsonrpc: String,
    id: String,
    method: String,
    params: Vec<String>,
}

#[derive(Serialize)]
struct JsonRequestRead {
    jsonrpc: String,
    id: String,
    method: String,
}

pub fn send_message(address: &String, message: &String) {
    let random_int_string_10_chars: String = thread_rng()
                .sample_iter(&rand::distributions::Uniform::new_inclusive(b'0', b'9'))
                .take(10)
                .map(|c| c as char)
                .collect();

    let message = message.to_string();

    let body = JsonRequestSend {
        jsonrpc: "2.0".to_string(),
        id: random_int_string_10_chars,
        method: "send".to_string(),
        params: vec![message],
    };

    let body_json = serde_json::to_string(&body).expect("Failed to serialize body to JSON");

    let http_client = Client::new();
    println!("{}", body_json);
    let post_result = http_client.post(address)
    .header("Content-Type", "application/json")
    .body(body_json)
    .send();

    match post_result {
        Ok(response) => {
            if response.status().is_success() {
                println!("Request successful");
            } else {
                println!("Request failed with status code: {}", response.status());
            }
        }
        Err(e) => {
            println!("Error sending request: {}", e);
        }
    }
}

pub fn read_message(address: &String) {
    let random_int_string_10_chars: String = thread_rng()
        .sample_iter(&rand::distributions::Uniform::new_inclusive(b'0', b'9'))
        .take(10)
        .map(|c| c as char)
        .collect();


    let body = JsonRequestRead {
    jsonrpc: "2.0".to_string(),
    id: random_int_string_10_chars,
    method: "read".to_string(),
    };

    let body_json = serde_json::to_string(&body).expect("Failed to serialize body to JSON");

    let http_client = Client::new();

    let post_result = http_client.post(address)
        .header("Content-Type", "application/json")
        .body(body_json)
        .send();

    match post_result {
    Ok(response) => {
            if response.status().is_success() {
                println!("Request successful");
            } else {
                println!("Request failed with status code: {}", response.status());
            }
        }
    Err(e) => {
            println!("Error sending request: {}", e);
        }
    }
}
