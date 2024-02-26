use std::time::Duration;
use std::thread;

use indicatif::ProgressBar;
use indicatif::ProgressStyle;

use rand::Rng;

use color_eyre::owo_colors::OwoColorize;

use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite:: WebSocketStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite:: tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

use futures_util::stream::SplitSink;
use futures_util::stream::SplitStream;
use futures_util::SinkExt;
use futures_util::StreamExt;

use crate::api;
use crate::output;

pub struct WSClientStream {
    pub write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pub read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>
}

impl WSClientStream {
    pub async fn get_stream(ws_address: &str) -> Result<Self, Box<dyn std::error::Error>> {
        println!("Connected to ws: {}", ws_address.green());
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

fn generate_request_method(method: &String) -> api::WsRequest {
    let id = generate_random_number();

    let command = match method.as_str() {
        "listRemoteApps" => api::ApiRequest::ListRemoteApps,
        "listInstalledApps" => api::ApiRequest::ListInstalledApps,
        "unsubscribeAll" => api::ApiRequest::UnsubscribeFromAll,
        _ => api::ApiRequest::ListRemoteApps
    };

    api::WsRequest {
        id: Some(id),
        command,
    }
}

fn generate_request_params(method: &String, params: Vec<u32>) -> api::WsRequest {
    let id = generate_random_number();

    let command = match method.as_str() {
        "installBinaryApp" => api::ApiRequest::InstallBinaryApp(params[0].to_be_bytes().to_vec()),
        "installRemoteApp" => api::ApiRequest::InstallRemoteApp(params[0]),
        "uninstallApp" => api::ApiRequest::UninstallApp(params[0]),
        "subscribe" => api::ApiRequest::Subscribe(params[0]),
        "unsubscribe" => api::ApiRequest::Unsubscribe(params[0]),
        _ => api::ApiRequest::InstallBinaryApp(params[0].to_be_bytes().to_vec())
    };

    api::WsRequest {
        id: Some(id),
        command,
    }
}

async fn close_connection(ws_client_stream: &mut WSClientStream ) {
    let close_code = CloseCode::Normal;
    let reason = String::from("End");

    let close_frame = CloseFrame {
        code: close_code.into(),
        reason: reason.clone().into(),
    };

    let message = Message::Close(Some(close_frame));

    ws_client_stream.write.send(message).await.expect("Failed to send message");
}

pub async fn list_remote_apps(ws_address: &String, method: &String) {
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
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::ListRemoteApps(apps) => {
                                        let asset = String::from("Remote Apps");
                                        let header: Vec<[&str; 2]> = vec![
                                            ["ID", "Description"]
                                        ];
                                        output::print_table_apps(&asset, &header, apps);
                                        
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    close_connection(&mut ws_client_stream).await;
}

pub async fn list_installed_apps(ws_address: &String, method: &String) {
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
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::ListInstalledApps(apps) => {
                                        let asset = String::from("Installed Apps");
                                        let header: Vec<[&str; 2]> = vec![
                                            ["ID", "Description"]
                                        ];
                                        output::print_table_installed_apps(&asset, &header, apps);
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    close_connection(&mut ws_client_stream).await;
}

pub async fn install_remote_app(ws_address: &String, method: &String, app_id: &u32) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");
    let params = vec![*app_id];
    let request_object = generate_request_params(method, params);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    let pb = ProgressBar::new_spinner();
    let style = ProgressStyle::default_spinner()
        .tick_chars("/|\\- ");

    let style = match style.template("{spinner:.green} {msg}") {
        Ok(style) => style,
        Err(e) => {
            eprintln!("Error setting progress bar template: {:?}", e);
            return;
        }
    };
    pb.set_style(style);

    loop {
        pb.enable_steady_tick(Duration::from_millis(100));

        for _ in 0..100 {
            pb.set_message("Loading...");
            thread::sleep(Duration::from_millis(50));
        }

        pb.disable_steady_tick();
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::InstallRemoteApp(app_id) => {
                                        println!("App with id: {} installed", app_id.green());
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    pb.finish_with_message("Application Installed!");
    close_connection(&mut ws_client_stream).await;
}

pub async fn install_binary_app(ws_address: &String, method: &String, binary_path: &String) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");
    //decide what to do here, read binary path and convert to vec<u8> ..
    println!("Application binary: {}", binary_path);
    let binary: u32 = 123;
    let params = vec![binary];
    let request_object = generate_request_params(method, params);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    let pb = ProgressBar::new_spinner();
    let style = ProgressStyle::default_spinner()
        .tick_chars("/|\\- ");

    let style = match style.template("{spinner:.green} {msg}") {
        Ok(style) => style,
        Err(e) => {
            eprintln!("Error setting progress bar template: {:?}", e);
            return;
        }
    };
    pb.set_style(style);

    loop {
        pb.enable_steady_tick(Duration::from_millis(100));

        for _ in 0..100 {
            pb.set_message("Loading...");
            thread::sleep(Duration::from_millis(50));
        }

        pb.disable_steady_tick();
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    // Handle progress responses and add loader
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::InstallBinaryApp(app_id) => {
                                        println!("App with id: {} installed", app_id.green());
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    pb.finish_with_message("Application Installed!");
    close_connection(&mut ws_client_stream).await;
}

pub async fn uninstall_app(ws_address: &String, method: &String, app_id: &u32) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");
    let params = vec![*app_id];
    let request_object = generate_request_params(method, params);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    let pb = ProgressBar::new_spinner();
    let style = ProgressStyle::default_spinner()
        .tick_chars("/|\\- ");

    let style = match style.template("{spinner:.green} {msg}") {
        Ok(style) => style,
        Err(e) => {
            eprintln!("Error setting progress bar template: {:?}", e);
            return;
        }
    };
    pb.set_style(style);

    loop {
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    // Handle progress response and add progress loader
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::UninstallApp(app_id) => {
                                        println!("App with id: {} uninstalled", app_id.green());
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    pb.finish_with_message("Application Uninstalled!");
    close_connection(&mut ws_client_stream).await;
}

pub async fn subscribe(ws_address: &String, method: &String, app_id: &u32) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");
    let params = vec![*app_id];
    let request_object = generate_request_params(method, params);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                //Handle ws messages
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::Subscribe(app_id) => {
                                        println!("Subscribed to App with id: {}", app_id.green());
                                        println!("Waiting for messages...");
                                        continue;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    close_connection(&mut ws_client_stream).await;
}

pub async fn unsubscribe(ws_address: &String, method: &String, app_id: &u32) {
    let mut ws_client_stream = WSClientStream::get_stream(ws_address)
        .await
        .expect("Failed to get WebSocket stream");
    let params = vec![*app_id];
    let request_object = generate_request_params(method, params);

    let json_string_reqest = serde_json::to_string(&request_object).expect("Failed to serialize JSON");

    let msg = Message::Text(r#json_string_reqest.to_string().into());

    ws_client_stream.write.send(msg).await.expect("Failed to send message");

    loop {
        if let Some(message) = ws_client_stream.read.next().await {
            if let Ok(text) = message.expect("Failed to read message").into_text() {
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::Unsubscribe(app_id) => {
                                        println!("Unsubscribed to App with id: {}", app_id.green());
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    close_connection(&mut ws_client_stream).await;
}

pub async fn unsubscribe_all(ws_address: &String, method: &String) {
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
                if let Ok(json_request) = serde_json::from_str::<api::WsResponse>(text.as_str()) {
                    let response_id = json_request.id.unwrap();
                    if response_id == request_object.id.unwrap() {
                        println!("Received response with id: {}",
                        response_id.green());
                        let result = json_request.result;

                        match result {
                            api::ApiResponseResult::Ok(response) => {
                                match response {
                                    api::ApiResponse::UnsubscribeFromAll => {
                                        println!("Unsubscribed from all.");
                                        break;
                                    }
                                    _ => {
                                        println!("Error - Unknown result");
                                        break;
                                    }
                                }
                            }
                            api::ApiResponseResult::Err(err) => {
                                println!("Error fetching data: {:?}", err);
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
    close_connection(&mut ws_client_stream).await;
}
