use std::borrow::Cow;
use std::collections::{hash_map, HashMap, HashSet};
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, MethodRouter};
use axum::Extension;
use calimero_server_primitives::ws as ws_primitives;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info};

#[derive(Debug, Serialize, Deserialize)]
pub struct WsConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

#[derive(Default)]
struct InnerState {
    connections: HashMap<ws_primitives::ClientId, mpsc::Sender<ws_primitives::Command>>,
    subscriptions: HashSet<ws_primitives::ClientId>,
}
struct ServiceState {
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    inner: RwLock<InnerState>,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
) -> eyre::Result<Option<(&'static str, MethodRouter)>> {
    let _config = match &config.websocket {
        Some(config) if config.enabled => config,
        _ => {
            info!("WebSocket server is disabled");
            return Ok(None);
        }
    };

    let path = "/ws"; // todo! source from config

    for listen in config.listen.iter() {
        info!("WebSocket server listening on {}/ws{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_events,
        inner: Default::default(),
    });

    Ok(Some((path, get(ws_handler).layer(Extension(state)))))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<ServiceState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<ServiceState>) {
    let (commands_sender, commands_receiver) = mpsc::channel(32);
    let client_id = loop {
        let client_id = rand::random();

        match state.inner.write().await.connections.entry(client_id) {
            hash_map::Entry::Occupied(_) => continue,
            hash_map::Entry::Vacant(entry) => {
                entry.insert(commands_sender.clone());
                break client_id;
            }
        }
    };

    debug!(%client_id, "Client connected");

    tokio::spawn(handle_node_events(
        client_id,
        state.clone(),
        state.node_events.subscribe(),
        commands_sender.clone(),
    ));

    let (socket_sender, mut socket_receiver) = socket.split();

    tokio::spawn(handle_commands(client_id, commands_receiver, socket_sender));

    while let Some(message) = socket_receiver.next().await {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                error!(%client_id, %e, "Failed to read ws::Message");
                break;
            }
        };

        match message {
            Message::Text(message) => {
                tokio::spawn(handle_text_message(client_id, state.clone(), message));
            }
            Message::Binary(_) => {
                debug!("Received binary message");
            }
            Message::Ping(_) => {
                debug!("Received ping message");
            }
            Message::Pong(_) => {
                debug!("Received pong message");
            }
            Message::Close(_) => {
                debug!("Received close message");
                break;
            }
        }
    }

    debug!(%client_id, "Client disconnected");
    let mut state = state.inner.write().await;
    state.subscriptions.remove(&client_id);
    state.connections.remove(&client_id);
}

async fn handle_node_events(
    client_id: ws_primitives::ClientId,
    state: Arc<ServiceState>,
    mut node_events_receiver: broadcast::Receiver<calimero_primitives::events::NodeEvent>,
    command_sender: mpsc::Sender<ws_primitives::Command>,
) {
    while let Ok(message) = node_events_receiver.recv().await {
        if state.inner.read().await.subscriptions.contains(&client_id) {
            let response = ws_primitives::Response {
                id: None,
                body: ws_primitives::ResonseBody::Result(ws_primitives::ResponseBodyResult::Event(
                    message,
                )),
            };

            if let Err(err) = command_sender
                .send(ws_primitives::Command::Send(response))
                .await
            {
                error!(
                    %client_id,
                    %err,
                    "Failed to send ws_primitives::WsCommand::Send",
                );
            }
        }
    }
}

async fn handle_commands(
    client_id: ws_primitives::ClientId,
    mut command_receiver: mpsc::Receiver<ws_primitives::Command>,
    mut socket_sender: SplitSink<WebSocket, Message>,
) {
    while let Some(action) = command_receiver.recv().await {
        match action {
            ws_primitives::Command::Close(code, reason) => {
                let close_frame = Some(CloseFrame {
                    code,
                    reason: Cow::from(reason),
                });
                if let Err(err) = socket_sender.send(Message::Close(close_frame)).await {
                    error!(
                        %client_id,
                        %err,
                        "Failed to send ws::Message::Close",
                    );
                }
                let _ = socket_sender.close().await;
                break;
            }
            ws_primitives::Command::Send(response) => {
                let response = match serde_json::to_string(&response) {
                    Ok(message) => message,
                    Err(err) => {
                        error!(
                            %client_id,
                            %err,
                            "Failed to serialize ws_primitives::WsResponse",
                        );
                        continue;
                    }
                };
                if let Err(err) = socket_sender.send(Message::Text(response)).await {
                    error!(
                        %client_id,
                        %err,
                        "Failed to send ws::Message::Text",
                    );
                }
            }
        }
    }
}

async fn handle_text_message(
    client_id: ws_primitives::ClientId,
    state: Arc<ServiceState>,
    message: String,
) {
    let response = match serde_json::from_str::<ws_primitives::Request>(&message) {
        Ok(message) => {
            let response_body = match message.body {
                ws_primitives::RequestBody::Subscribe => {
                    state.inner.write().await.subscriptions.insert(client_id);
                    ws_primitives::ResponseBodyResult::Subscribed
                }
                ws_primitives::RequestBody::Unsubscribe => {
                    state.inner.write().await.subscriptions.remove(&client_id);
                    ws_primitives::ResponseBodyResult::Unsubscribed
                }
            };
            ws_primitives::Response {
                id: message.id,
                body: ws_primitives::ResonseBody::Result(response_body),
            }
        }
        Err(err) => {
            error!(%message, %err, "Failed to deserialize ws_primitives::WsRequest");

            let error_body = ws_primitives::ResponseBodyError::SerdeError(String::from(format!(
                "failed to deserialize request: {message}"
            )));
            ws_primitives::Response {
                id: None,
                body: ws_primitives::ResonseBody::Error(error_body),
            }
        }
    };

    if let Some(sender) = state.inner.read().await.connections.get(&client_id) {
        if let Err(err) = sender.send(ws_primitives::Command::Send(response)).await {
            error!(
                %client_id,
                %err,
                "Failed to send ws_primitives::WsCommand::Send",
            );
        }
    };
}
