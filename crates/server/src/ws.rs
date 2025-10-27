use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, MethodRouter};
use axum::Extension;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::ws::{
    Command, ConnectionId, Request as WsRequest, RequestPayload, Response, ResponseBody,
    ResponseBodyError, ServerResponseError,
};
use eyre::Error as EyreError;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::{
    from_str as from_json_str, from_value as from_json_value, to_string as to_json_string,
    to_value as to_json_value, Value,
};
use tokio::spawn;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

mod subscribe;
mod unsubscribe;

/// WebSocket close codes (RFC 6455)
/// https://datatracker.ietf.org/doc/html/rfc6455#section-7.4.1
mod close_code {
    /// Endpoint is terminating the connection due to a protocol error (e.g., ping timeout).
    pub const PROTOCOL_ERROR: u16 = 1002;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct WsConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,

    /// Interval for server-initiated ping messages (in seconds)
    /// Set to 0 to disable server pings (rely on client pings only)
    #[serde(default = "default_ping_interval")]
    pub ping_interval_secs: u64,

    /// Timeout for pong responses (in seconds)
    /// If no pong received within this time after ping, connection is closed
    #[serde(default = "default_pong_timeout")]
    pub pong_timeout_secs: u64,
}

const fn default_ping_interval() -> u64 {
    30 // Send ping every 30 seconds
}

const fn default_pong_timeout() -> u64 {
    10 // Expect pong within 10 seconds
}

impl WsConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ping_interval_secs: default_ping_interval(),
            pong_timeout_secs: default_pong_timeout(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ConnectionStateInner {
    subscriptions: HashSet<ContextId>,
    last_pong: AtomicU64, // Timestamp of last received pong (or connection start)
}

impl Default for ConnectionStateInner {
    fn default() -> Self {
        Self {
            subscriptions: HashSet::default(),
            last_pong: AtomicU64::new(unix_timestamp()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionState {
    commands: mpsc::Sender<Command>,
    inner: Arc<RwLock<ConnectionStateInner>>,
}

pub(crate) struct ServiceState {
    node_client: NodeClient,
    connections: RwLock<HashMap<ConnectionId, ConnectionState>>,
    config: WsConfig,
}

/// Get current Unix timestamp in seconds
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}

pub(crate) fn service(
    config: &ServerConfig,
    node_client: NodeClient,
) -> Option<(String, MethodRouter)> {
    let ws_config = match &config.websocket {
        Some(config) if config.enabled => *config,
        _ => {
            info!("WebSocket server is disabled");
            return None;
        }
    };

    let base_path = "/ws";

    // Get the node prefix from env var
    let path = if let Ok(prefix) = std::env::var("NODE_PATH_PREFIX") {
        format!("{}{}", prefix, base_path)
    } else {
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!("WebSocket server listening on {}/ws{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_client,
        connections: RwLock::default(),
        config: ws_config,
    });

    Some((path, get(ws_handler).layer(Extension(state))))
}

async fn ws_handler(
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    Extension(state): Extension<Arc<ServiceState>>,
) -> impl IntoResponse {
    // Validate WebSocket upgrade request
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => {
            debug!("Invalid WebSocket upgrade request: {}", rejection);
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid WebSocket upgrade request: {}", rejection),
            )
                .into_response();
        }
    };

    // Check for required upgrade headers
    if !headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        debug!("Missing or invalid upgrade header");
        return (
            StatusCode::UPGRADE_REQUIRED,
            "This endpoint requires WebSocket upgrade",
        )
            .into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: Arc<ServiceState>) {
    let (commands_sender, commands_receiver) = mpsc::channel(WS_COMMAND_CHANNEL_BUFFER_SIZE);
    let (connection_id, connection_state) = loop {
        let connection_id = random();
        let mut connections = state.connections.write().await;

        match connections.entry(connection_id) {
            Entry::Occupied(_) => continue,
            Entry::Vacant(entry) => {
                let connection_state = ConnectionState {
                    commands: commands_sender.clone(),
                    inner: Arc::default(),
                };
                let _ = entry.insert(connection_state.clone());
                break (connection_id, connection_state);
            }
        }
    };

    debug!(%connection_id, "Client connection established");

    drop(spawn(handle_node_events(
        connection_id,
        Arc::clone(&state),
        commands_sender.clone(),
    )));

    let (socket_sender, mut socket_receiver) = socket.split();

    drop(spawn(handle_commands(
        connection_id,
        commands_receiver,
        socket_sender,
    )));

    // Spawn health check task if ping interval is configured
    if state.config.ping_interval_secs > 0 {
        drop(spawn(handle_health_check(
            connection_id,
            Arc::clone(&state),
            connection_state.clone(),
        )));
    }

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
                drop(spawn(handle_text_message(
                    connection_id,
                    Arc::clone(&state),
                    message,
                )));
            }
            Message::Binary(_) => {
                debug!("Received binary message");
            }
            Message::Ping(payload) => {
                debug!(%connection_id, "Received ping message, responding with pong");
                // Respond to ping with pong to keep connection alive
                if let Err(err) = connection_state.commands.send(Command::Pong(payload)).await {
                    error!(%connection_id, %err, "Failed to send pong response");
                }
            }
            Message::Pong(_) => {
                debug!(%connection_id, "Received pong message");
                // Update last pong timestamp for health monitoring
                connection_state
                    .inner
                    .read()
                    .await
                    .last_pong
                    .store(unix_timestamp(), Ordering::Relaxed);
            }
            Message::Close(close_frame) => {
                if let Some(frame) = close_frame {
                    info!(
                        %connection_id,
                        code = frame.code,
                        reason = %frame.reason,
                        "Received close message from client"
                    );
                } else {
                    info!(%connection_id, "Received close message from client (no close frame)");
                }
                break;
            }
        }
    }

    debug!(%connection_id, "Client connection terminated");

    let mut state = state.connections.write().await;
    drop(state.remove(&connection_id));
}

async fn handle_node_events(
    connection_id: ConnectionId,
    state: Arc<ServiceState>,
    command_sender: mpsc::Sender<Command>,
) {
    let events = state.node_client.receive_events();

    let mut events = pin!(events);

    while let Some(event) = events.next().await {
        let Some(connection_state) = state.connections.read().await.get(&connection_id).cloned()
        else {
            error!(%connection_id, "Unexpected state, client_id not found in client state map");
            return;
        };

        debug!(
            %connection_id,
            "Received node event: {:?}, subscriptions state: {:?}",
            event,
            connection_state.inner.read().await.subscriptions
        );

        let event = match event {
            NodeEvent::Context(event)
                if {
                    connection_state
                        .inner
                        .read()
                        .await
                        .subscriptions
                        .contains(&event.context_id)
                } =>
            {
                NodeEvent::Context(event)
            }
            NodeEvent::Context(_) => continue,
        };

        let body = match to_json_value(event) {
            Ok(v) => ResponseBody::Result(v),
            Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::InternalError {
                    err: Some(err.into()),
                },
            )),
        };

        let response = Response { id: None, body };

        if let Err(err) = command_sender.send(Command::Send(response)).await {
            error!(
                %connection_id,
                %err,
                "Failed to send WsCommand::Send",
            );
        };
    }
}

async fn handle_commands(
    connection_id: ConnectionId,
    mut command_receiver: mpsc::Receiver<Command>,
    mut socket_sender: SplitSink<WebSocket, Message>,
) {
    while let Some(action) = command_receiver.recv().await {
        match action {
            Command::Close(code, reason) => {
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
                if let Err(err) = socket_sender.close().await {
                    debug!(
                        %connection_id,
                        %err,
                        "Error closing WebSocket sender (connection likely already closed)",
                    );
                }
                break;
            }
            Command::Send(response) => {
                let response = match to_json_string(&response) {
                    Ok(message) => message,
                    Err(err) => {
                        error!(
                            %connection_id,
                            %err,
                            "Failed to serialize WsResponse",
                        );
                        continue;
                    }
                };
                if let Err(err) = socket_sender.send(Message::Text(response)).await {
                    error!(%connection_id, %err, "Failed to send Message::Text");
                }
            }
            Command::Ping(payload) => {
                if let Err(err) = socket_sender.send(Message::Ping(payload)).await {
                    error!(%connection_id, %err, "Failed to send ws::Message::Ping");
                }
            }
            Command::Pong(payload) => {
                if let Err(err) = socket_sender.send(Message::Pong(payload)).await {
                    error!(%connection_id, %err, "Failed to send ws::Message::Pong");
                }
            }
        }
    }
}

async fn handle_health_check(
    connection_id: ConnectionId,
    state: Arc<ServiceState>,
    connection_state: ConnectionState,
) {
    let mut ping_timer = interval(Duration::from_secs(state.config.ping_interval_secs));
    let _ = ping_timer.tick().await; // First tick completes immediately

    loop {
        let _ = ping_timer.tick().await;

        // Check if connection still exists
        if state.connections.read().await.get(&connection_id).is_none() {
            debug!(%connection_id, "Connection no longer exists, stopping health check");
            break;
        }

        // Check for pong timeout
        let last_pong = connection_state
            .inner
            .read()
            .await
            .last_pong
            .load(Ordering::Relaxed);
        let elapsed = unix_timestamp().saturating_sub(last_pong);

        if elapsed > state.config.ping_interval_secs + state.config.pong_timeout_secs {
            warn!(
                %connection_id,
                elapsed_secs = elapsed,
                timeout_secs = state.config.ping_interval_secs + state.config.pong_timeout_secs,
                "Client failed to respond to ping, closing connection"
            );

            // Close connection due to timeout
            if let Err(err) = connection_state
                .commands
                .send(Command::Close(
                    close_code::PROTOCOL_ERROR,
                    "Ping timeout".to_owned(),
                ))
                .await
            {
                error!(%connection_id, %err, "Failed to send close command");
            }
            break;
        }

        // Send ping to check if client is alive
        debug!(%connection_id, "Sending ping to client");
        if let Err(err) = connection_state.commands.send(Command::Ping(vec![])).await {
            error!(%connection_id, %err, "Failed to send ping command");
            break;
        }
    }
}

async fn handle_text_message(
    connection_id: ConnectionId,
    state: Arc<ServiceState>,
    message: String,
) {
    debug!(%connection_id, %message, "Received text message");
    let Some(connection_state) = state.connections.read().await.get(&connection_id).cloned() else {
        error!(%connection_id, "Connection not found in state map");
        return;
    };

    let message = match from_json_str::<WsRequest<Value>>(&message) {
        Ok(message) => message,
        Err(err) => {
            error!(%connection_id, %err, "Failed to deserialize Request<Value>");
            return;
        }
    };

    let body = match from_json_value::<RequestPayload>(message.payload) {
        Ok(payload) => match payload {
            RequestPayload::Subscribe(request) => request
                .handle(Arc::clone(&state), connection_state.clone())
                .await
                .to_res_body(),
            RequestPayload::Unsubscribe(request) => request
                .handle(Arc::clone(&state), connection_state.clone())
                .await
                .to_res_body(),
        },
        Err(err) => {
            error!(%connection_id, %err, "Failed to deserialize RequestPayload");

            ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::ParseError(err.to_string()),
            ))
        }
    };

    if let Err(err) = connection_state
        .commands
        .send(Command::Send(Response {
            id: message.id,
            body,
        }))
        .await
    {
        error!(
            %connection_id,
            %err,
            "Failed to send WsCommand::Send",
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
#[non_exhaustive]
pub enum WsError<E> {
    MethodCallError(E),
    InternalError(EyreError),
}

trait ToResponseBody {
    fn to_res_body(self) -> ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, WsError<E>> {
    fn to_res_body(self) -> ResponseBody {
        match self {
            Ok(r) => match to_json_value(r) {
                Ok(v) => ResponseBody::Result(v),
                Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError {
                        err: Some(err.into()),
                    },
                )),
            },
            Err(WsError::MethodCallError(err)) => match to_json_value(err) {
                Ok(v) => ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError {
                        err: Some(err.into()),
                    },
                )),
            },
            Err(WsError::InternalError(err)) => {
                ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: Some(err) },
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
            ) -> core::result::Result<Self::Response, crate::ws::WsError<Self::Error>> {
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

use crate::config::ServerConfig;

/// WebSocket command channel buffer size
///
/// This controls how many WebSocket commands can be queued in the channel before
/// the sender blocks. Should match SSE's COMMAND_CHANNEL_BUFFER_SIZE for consistency.
const WS_COMMAND_CHANNEL_BUFFER_SIZE: usize = 32;
