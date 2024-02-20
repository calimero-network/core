use rand::Rng;

use color_eyre::owo_colors::OwoColorize;

use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use futures_util::{stream::{SplitSink, SplitStream}, SinkExt, StreamExt};

use crate::{api::{ApiRequest, ApiResponse, WsRequest, WsResponse}, output};

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

fn generate_request_method(method: &String) -> WsRequest {
    let id = generate_random_number();

    let command = match method.as_str() {
        "listRemoteApps" => ApiRequest::ListRemoteApps(),
        "listInstalledApps" => ApiRequest::ListInstalledApps(),
        "unsubscribeAll" => ApiRequest::UnsubscribeFromAll(),
        _ => ApiRequest::ListRemoteApps()
    };

    WsRequest {
        id: Some(id),
        command,
    }
}

fn generate_request_params(method: &String, params: Vec<u32>) -> WsRequest {
    let id = generate_random_number();

    let command = match method.as_str() {
        "installBinaryApp" => ApiRequest::InstallBinaryApp(params[0].to_be_bytes().to_vec()),
        "installRemoteApp" => ApiRequest::InstallRemoteApp(params[0]),
        "uninstallApp" => ApiRequest::UninstallApp(params[0]),
        "subscribe" => ApiRequest::Subscribe(params[0]),
        "unsubscribe" => ApiRequest::Unsubscribe(params[0]),
        _ => ApiRequest::InstallBinaryApp(params[0].to_be_bytes().to_vec())
    };
    
    WsRequest {
        id: Some(id),
        command,
    }
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
                if let Ok(json_request) = serde_json::from_str::<WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id);
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
                println!("msg: {}", text.as_str());
                if let Ok(json_request) = serde_json::from_str::<WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id);
                        break;
                    } 
                } else {
                   continue;
                }
            }
        }
    }
}


pub async fn list_remote_apps(ws_address: &String, method: &String) {
    // TODO - error handling and program exit
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
                if let Ok(json_request) = serde_json::from_str::<WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            Ok(response) => {
                                match response {
                                    ApiResponse::ListRemoteApps(apps) => {
                                        let asset = String::from("Remote Apps");
                                        let header: Vec<[&str; 2]> = vec![
                                                ["ID", "Description"]
                                            ];
                                        output::print_table_apps(&asset, &header, apps);
                                        return;
                                    }
                                    _ => {}
                                }
                            },
                            Err(err) => {
                                println!("Error fetching data: {}", err);
                                continue;
                            }
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        }
    }
}

pub async fn list_installed_apps(ws_address: &String, method: &String) {
    // TODO - error handling and program exit
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
                if let Ok(json_request) = serde_json::from_str::<WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            Ok(response) => {
                                match response {
                                    ApiResponse::ListInstalledApps(apps) => {
                                        let asset = String::from("Remote Apps");
                                        let header: Vec<[&str; 2]> = vec![
                                                ["ID", "Description"]
                                            ];
                                        output::print_table_installed_apps(&asset, &header, apps);
                                        return;
                                    }
                                    _ => {}
                                }
                            },
                            Err(err) => {
                                println!("Error fetching data: {}", err);
                                continue;
                            }
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        }
    }
}