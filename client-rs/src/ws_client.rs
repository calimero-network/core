use rand::Rng;

use serde::{Deserialize, Serialize};
use color_eyre::owo_colors::OwoColorize;

use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use futures_util::{stream::{SplitSink, SplitStream}, SinkExt, StreamExt};

use crate::commands;

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

fn generate_random_number() -> u32 {
    let mut rng = rand::thread_rng();
    rng.gen_range(100_000..=1_000_000)
}

///
/// Here for request method we send commands::WsCommand items
/// ID is needed for knowing which response we looping for (u8 or u32) number
fn generate_request_method(method: &String) -> commands::WsCommand {
    let id = generate_random_number();

    let request = match method.as_str() {
        "listApps" => commands::WsCommand::ListApps(),
        "listPods" => commands::WsCommand::ListApps(),
        _ => commands::WsCommand::ListApps()
    };
    request
}

fn generate_request_params(method: &String, params: Vec<u32>) -> commands::WsCommand {
    let id = generate_random_number();

    let request = match method.as_str() {
        "startPod" => commands::WsCommand::StartPod(params[0]),
        "stopPod" => commands::WsCommand::StopPod(params[0]),
        "subscribe" => commands::WsCommand::Subscribe(params[0]),
        "unsubscribe" => commands::WsCommand::Unsubscribe(params[0]),
        _ => commands::WsCommand::ListApps()
    };
    request
}

pub async fn ws_params(ws_address: &String, method: &String, params: Vec<u32>) {
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
                println!("msg: {}", text.as_str());
                // Response will be json string that will need to be serialized
                // Type will be JsonResponseWebsocket or similar name
                // will include result + error + id
                // for result it will be response for each application
                // we will know which type it is with match and recording to that
                // parse it to right value
                // for example Apps we know the struct
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSendParams>(text.as_str()) {
                    // if json_request.id == request_object.id {
                    //     println!("Received response with id: {} \nMessage received: {}",
                    //     json_request.id, json_request.params.join(" ").green());
                    //     break;
                    // } 
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
                println!("msg: {}", text.as_str());
                if let Ok(json_request) = serde_json::from_str::<JsonRequestSendMethod>(text.as_str()) {
                    
                    // if json_request.id == request_object.id {
                    //     println!("Received response with id: {}", json_request.id);
                    //     println!("Message received: {}", json_request.method.green());
                    //     break;
                    // }
                } else {
                   continue;
                }
            }
        }
    }
}


// TO DO - FUNCTIONS FOR EACH