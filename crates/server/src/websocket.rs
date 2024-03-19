use std::borrow::Cow;
use std::collections::{hash_map, HashMap, HashSet};
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, MethodRouter};
use axum::Extension;
use calimero_primitives::server::{self, WsCommand};
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
    connections: HashMap<server::WsClientId, mpsc::Sender<server::WsCommand>>,
    subscriptions: HashSet<calimero_primitives::server::WsClientId>,
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

    info!(%client_id, "new client connected");

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
                error!(%client_id, %e, "failed to read ws::Message");
                break;
            }
        };

        match message {
            Message::Text(message) => {
                tokio::spawn(handle_text_message(client_id, state.clone(), message));
            }
            Message::Binary(_) => {
                debug!("received binary message");
            }
            Message::Ping(_) => {
                debug!("received ping message");
            }
            Message::Pong(_) => {
                debug!("received pong message");
            }
            Message::Close(_) => {
                debug!("received close message");
                break;
            }
        }
    }

    info!(%client_id, "client disconnected");
    let mut state = state.inner.write().await;
    state.subscriptions.remove(&client_id);
    state.connections.remove(&client_id);
}

async fn handle_node_events(
    client_id: server::WsClientId,
    state: Arc<ServiceState>,
    mut node_events_receiver: broadcast::Receiver<calimero_primitives::events::NodeEvent>,
    command_sender: mpsc::Sender<WsCommand>,
) {
    while let Ok(message) = node_events_receiver.recv().await {
        if state.inner.read().await.subscriptions.contains(&client_id) {
            let response = server::WsResponse {
                id: None,
                body: server::WsResonseBody::Result(server::WsResponseBodyResult::Event(message)),
            };

            if let Err(e) = command_sender.send(server::WsCommand::Send(response)).await {
                error!(
                    %client_id,
                    %e,
                    "failed to send server::WsCommand::Send",
                );
            }
        }
    }
}

async fn handle_commands(
    client_id: server::WsClientId,
    mut command_receiver: mpsc::Receiver<WsCommand>,
    mut socket_sender: SplitSink<WebSocket, Message>,
) {
    while let Some(action) = command_receiver.recv().await {
        match action {
            server::WsCommand::Close(code, reason) => {
                let close_frame = Some(CloseFrame {
                    code,
                    reason: Cow::from(reason),
                });
                if let Err(e) = socket_sender.send(Message::Close(close_frame)).await {
                    error!(
                        %client_id,
                        %e,
                        "failed to send ws::Message::Close",
                    );
                }
                let _ = socket_sender.close().await;
                break;
            }
            server::WsCommand::Send(response) => {
                let response = match serde_json::to_string(&response) {
                    Ok(message) => message,
                    Err(e) => {
                        error!(
                            %client_id,
                            %e,
                            "failed to serialize server::WsResponse",
                        );
                        continue;
                    }
                };
                if let Err(e) = socket_sender.send(Message::Text(response)).await {
                    error!(
                        %client_id,
                        %e,
                        "failed to send ws::Message::Text",

                    );
                }
            }
        }
    }
}

async fn handle_text_message(
    client_id: server::WsClientId,
    state: Arc<ServiceState>,
    message: String,
) {
    let response = match serde_json::from_str::<calimero_primitives::server::WsRequest>(&message) {
        Ok(message) => {
            let response_body = match message.body {
                server::WsRequestBody::Subscribe => {
                    state.inner.write().await.subscriptions.insert(client_id);
                    server::WsResponseBodyResult::Subscribed
                }
                server::WsRequestBody::Unsubscribe => {
                    state.inner.write().await.subscriptions.remove(&client_id);
                    server::WsResponseBodyResult::Unsubscribed
                }
            };
            server::WsResponse {
                id: message.id,
                body: server::WsResonseBody::Result(response_body),
            }
        }
        Err(e) => {
            error!(%message, %e, "failed to deserialize server::WsRequest");

            let error_body = server::WsError::SerdeError(String::from(format!(
                "failed to deserialize request: {message}"
            )));
            server::WsResponse {
                id: None,
                body: server::WsResonseBody::Error(error_body),
            }
        }
    };

    if let Some(sender) = state.inner.read().await.connections.get(&client_id) {
        if let Err(e) = sender.send(server::WsCommand::Send(response)).await {
            error!(
                %client_id,
                %e,
                "failed send server::WsCommand::Send",
            );
        }
    };
}
