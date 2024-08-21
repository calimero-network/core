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

mod subscribe;
mod unsubscribe;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct WsConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionStateInner {
    subscriptions: HashSet<calimero_primitives::context::ContextId>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionState {
    commands: mpsc::Sender<ws_primitives::Command>,
    inner: Arc<RwLock<ConnectionStateInner>>,
}

pub(crate) struct ServiceState {
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    connections: RwLock<HashMap<ws_primitives::ConnectionId, ConnectionState>>,
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
        connections: Default::default(),
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
    let (connection_id, _) = loop {
        let connection_id = rand::random();

        match state.connections.write().await.entry(connection_id) {
            hash_map::Entry::Occupied(_) => continue,
            hash_map::Entry::Vacant(entry) => {
                let connection_state = ConnectionState {
                    commands: commands_sender.clone(),
                    inner: Default::default(),
                };
                let _ = entry.insert(connection_state.clone());
                break (connection_id, connection_state);
            }
        }
    };

    debug!(%connection_id, "Client connection established");

    drop(tokio::spawn(handle_node_events(
        connection_id,
        state.clone(),
        state.node_events.subscribe(),
        commands_sender.clone(),
    )));

    let (socket_sender, mut socket_receiver) = socket.split();

    drop(tokio::spawn(handle_commands(
        connection_id,
        commands_receiver,
        socket_sender,
    )));

    while let Some(message) = socket_receiver.next().await {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                error!(%connection_id, %e, "Failed to read ws::Message");
                break;
            }
        };

        match message {
            Message::Text(message) => {
                drop(tokio::spawn(handle_text_message(
                    connection_id,
                    state.clone(),
                    message,
                )));
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

    debug!(%connection_id, "Client connection terminated");

    let mut state = state.connections.write().await;
    drop(state.remove(&connection_id));
}

async fn handle_node_events(
    connection_id: ws_primitives::ConnectionId,
    state: Arc<ServiceState>,
    mut node_events_receiver: broadcast::Receiver<calimero_primitives::events::NodeEvent>,
    command_sender: mpsc::Sender<ws_primitives::Command>,
) {
    while let Ok(event) = node_events_receiver.recv().await {
        let connections = state.connections.read().await;
        let connection_state = match connections.get(&connection_id) {
            Some(state) => state,
            None => {
                error!(%connection_id, "Unexpected state, client_id not found in client state map");
                return;
            }
        };

        debug!(
            %connection_id,
            "Received node event: {:?}, subscriptions state: {:?}",
            event,
            connection_state.inner.read().await.subscriptions
        );

        let event = match event {
            calimero_primitives::events::NodeEvent::Application(event)
                if {
                    connection_state
                        .inner
                        .read()
                        .await
                        .subscriptions
                        .contains(&event.context_id)
                } =>
            {
                calimero_primitives::events::NodeEvent::Application(event)
            }
            _ => continue,
        };

        let body = match serde_json::to_value(event) {
            Ok(v) => ws_primitives::ResponseBody::Result(v),
            Err(err) => {
                ws_primitives::ResponseBody::Error(ws_primitives::ResponseBodyError::ServerError(
                    ws_primitives::ServerResponseError::InternalError {
                        err: Some(err.into()),
                    },
                ))
            }
        };

        let response = ws_primitives::Response { id: None, body };

        if let Err(err) = command_sender
            .send(ws_primitives::Command::Send(response))
            .await
        {
            error!(
                %connection_id,
                %err,
                "Failed to send ws_primitives::WsCommand::Send",
            );
        };
    }
}

async fn handle_commands(
    connection_id: ws_primitives::ConnectionId,
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
                        %connection_id,
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
                            %connection_id,
                            %err,
                            "Failed to serialize ws_primitives::WsResponse",
                        );
                        continue;
                    }
                };
                if let Err(err) = socket_sender.send(Message::Text(response)).await {
                    error!(%connection_id, %err, "Failed to send ws::Message::Text");
                }
            }
        }
    }
}

async fn handle_text_message(
    connection_id: ws_primitives::ConnectionId,
    state: Arc<ServiceState>,
    message: String,
) {
    debug!(%connection_id, %message, "Received text message");
    let connections = state.connections.read().await;
    let connection_state = match connections.get(&connection_id) {
        Some(state) => state,
        None => {
            error!(%connection_id, "Unexpected state, client_id not found in client state map");
            return;
        }
    };

    if state.connections.read().await.get(&connection_id).is_none() {
        error!(%connection_id, "Unexpected state, client_id not found in client state map");
        return;
    };

    let message = match serde_json::from_str::<ws_primitives::Request<serde_json::Value>>(&message)
    {
        Ok(message) => message,
        Err(err) => {
            error!(%connection_id, %err, "Failed to deserialize ws_primitives::Request<serde_json::Value>");
            return;
        }
    };

    let body = match serde_json::from_value::<ws_primitives::RequestPayload>(message.payload) {
        Ok(payload) => match payload {
            ws_primitives::RequestPayload::Subscribe(request) => request
                .handle(state.clone(), connection_state.clone())
                .await
                .to_res_body(),
            ws_primitives::RequestPayload::Unsubscribe(request) => request
                .handle(state.clone(), connection_state.clone())
                .await
                .to_res_body(),
        },
        Err(err) => {
            error!(%connection_id, %err, "Failed to deserialize ws_primitives::RequestPayload");

            ws_primitives::ResponseBody::Error(ws_primitives::ResponseBodyError::ServerError(
                ws_primitives::ServerResponseError::ParseError(err.to_string()),
            ))
        }
    };

    if let Err(err) = connection_state
        .commands
        .send(ws_primitives::Command::Send(ws_primitives::Response {
            id: message.id,
            body,
        }))
        .await
    {
        error!(
            %connection_id,
            %err,
            "Failed to send ws_primitives::WsCommand::Send",
        );
    };
}

pub(crate) trait Request {
    type Response;
    type Error;

    async fn handle(
        self,
        _state: Arc<ServiceState>,
        connection_state: ConnectionState,
    ) -> Result<Self::Response, WsError<Self::Error>>;
}

#[derive(Debug)]
pub enum WsError<E> {
    MethodCallError(E),
    InternalError(eyre::Error),
}

trait ToResponseBody {
    fn to_res_body(self) -> ws_primitives::ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, WsError<E>> {
    fn to_res_body(self) -> ws_primitives::ResponseBody {
        match self {
            Ok(r) => match serde_json::to_value(r) {
                Ok(v) => ws_primitives::ResponseBody::Result(v),
                Err(err) => ws_primitives::ResponseBody::Error(
                    ws_primitives::ResponseBodyError::ServerError(
                        ws_primitives::ServerResponseError::InternalError {
                            err: Some(err.into()),
                        },
                    ),
                ),
            },
            Err(WsError::MethodCallError(err)) => match serde_json::to_value(err) {
                Ok(v) => ws_primitives::ResponseBody::Error(
                    ws_primitives::ResponseBodyError::HandlerError(v),
                ),
                Err(err) => ws_primitives::ResponseBody::Error(
                    ws_primitives::ResponseBodyError::ServerError(
                        ws_primitives::ServerResponseError::InternalError {
                            err: Some(err.into()),
                        },
                    ),
                ),
            },
            Err(WsError::InternalError(err)) => {
                ws_primitives::ResponseBody::Error(ws_primitives::ResponseBodyError::ServerError(
                    ws_primitives::ServerResponseError::InternalError { err: Some(err) },
                ))
            }
        }
    }
}

macro_rules! mount_method {
    ($request:ident -> Result<$response:ident, $error:ident>, $handle:path) => {
        impl crate::ws::Request for $request {
            type Response = $response;
            type Error = $error;

            async fn handle(
                self,
                state: std::sync::Arc<crate::ws::ServiceState>,
                connection_state: crate::ws::ConnectionState,
            ) -> std::result::Result<Self::Response, crate::ws::WsError<Self::Error>> {
                match $handle(self, state, connection_state).await {
                    Ok(response) => Ok(response),
                    Err(err) => match err.downcast::<Self::Error>() {
                        Ok(err) => Err(crate::ws::WsError::MethodCallError(err)),
                        Err(err) => Err(crate::ws::WsError::InternalError(err)),
                    },
                }
            }
        }
    };
}

pub(crate) use mount_method;
