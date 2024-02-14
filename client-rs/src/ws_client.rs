use futures_util::{SinkExt, StreamExt};
use rand::{Rng, thread_rng};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use color_eyre::owo_colors::OwoColorize;

#[derive(Debug, Serialize, Deserialize)]
struct JsonRequestSendParams {
    jsonrpc: String,
    id: String,
    method: String,
    params: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRequestSend {
    jsonrpc: String,
    id: String,
    method: String,
}

pub async fn ws_params(ws_address: &String, method: &String, params: Vec<String>) {

    println!("Connecting to {}", ws_address.green());

    let (ws_stream, _) = connect_async(ws_address).await.expect("Failed to connect");

    let (mut write, mut read) = ws_stream.split();

    let random_int_string_10_chars: String = thread_rng()
    .sample_iter(&rand::distributions::Uniform::new_inclusive(b'0', b'9'))
    .take(10)
    .map(|c| c as char)
    .collect();

    
    let request_object = JsonRequestSendParams {
        jsonrpc: "2.0".to_string(),
        id: random_int_string_10_chars.clone(),
        method: method.to_string(),
        params
    };
    
    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");
    

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSendParams>(text.as_str()) {
                    if json_request.id == random_int_string_10_chars {
                        println!("Received response with id: {}", json_request.id);
                        println!("Message received: {}", json_request.params.join(" ").green());
                        break;
                    }
                } else {
                    //
                }
            }
        }
    }
}

pub async fn ws_no_params(ws_address: &String, method: &String) {

    println!("Connecting to {}", ws_address.green());

    let (ws_stream, _) = connect_async(ws_address).await.expect("Failed to connect");

    let (mut write, mut read) = ws_stream.split();

    let random_int_string_10_chars: String = thread_rng()
    .sample_iter(&rand::distributions::Uniform::new_inclusive(b'0', b'9'))
    .take(10)
    .map(|c| c as char)
    .collect();

    
    let request_object = JsonRequestSend {
        jsonrpc: "2.0".to_string(),
        id: random_int_string_10_chars.clone(),
        method: method.to_string(),
    };
    
    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");
    

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSend>(text.as_str()) {
                    if json_request.id == random_int_string_10_chars {
                        println!("Received response with id: {}", json_request.id);
                        println!("Message received: {}", json_request.method.green());
                        break;
                    }
                } else {
                    //
                }
            }
        }
    }
}
