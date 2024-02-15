use rand::{Rng, thread_rng};

use serde::{Deserialize, Serialize};
use color_eyre::owo_colors::OwoColorize;

use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use futures_util::{stream::{SplitSink, SplitStream}, SinkExt, StreamExt};

#[derive(Debug, Serialize, Deserialize)]
struct JsonRequestSendParams {
    jsonrpc: String,
    id: String,
    method: String,
    params: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRequestSendMethod {
    jsonrpc: String,
    id: String,
    method: String,
}

pub struct WSClientStream {
    pub write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pub read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>
}

impl WSClientStream {
    pub async fn get_stream(ws_address: &str) -> Result<Self, Box<dyn std::error::Error>> {
        println!("Connecting to {}", ws_address.green());
        let (ws_stream, _) = connect_async(ws_address).await?;
        let (write,read) = ws_stream.split();
        Ok(Self {
            write,
            read
         })
    }
}

fn generate_request_method(method: &String) -> JsonRequestSendMethod {
    let random_int_string_10_chars: String = thread_rng()
    .sample_iter(&rand::distributions::Uniform::new_inclusive(b'0', b'9'))
    .take(10)
    .map(|c| c as char)
    .collect();

    let request_object = JsonRequestSendMethod {
        jsonrpc: "2.0".to_string(),
        id: random_int_string_10_chars.clone(),
        method: method.to_string(),
    };

    request_object
}

fn generate_request_params(method: &String, params: Vec<String>) -> JsonRequestSendParams {
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

    request_object
}

pub async fn ws_params(ws_address: &String, method: &String, params: Vec<String>) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");

    let request_object = generate_request_params(method, params);
    
    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");
    
    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSendParams>(text.as_str()) {
                    if json_request.id == request_object.id {
                        println!("Received response with id: {} \nMessage received: {}",
                        json_request.id, json_request.params.join(" ").green());
                        break;
                    }
                } else {
                    continue
                }
            }
        }
    }
}

pub async fn ws_no_params(ws_address: &String, method: &String) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");

    let request_object = generate_request_method(method);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSendMethod>(text.as_str()) {
                    if json_request.id == request_object.id {
                        println!("Received response with id: {}", json_request.id);
                        println!("Message received: {}", json_request.method.green());
                        break;
                    }
                } else {
                   continue;
                }
            }
        }
    }
}
