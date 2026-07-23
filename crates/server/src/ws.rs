use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, MethodRouter};
use axum::Extension;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::ws::{
    Command, Request as WsRequest, RequestPayload, Response, ResponseBody, ResponseBodyError,
    ServerResponseError,
};
use eyre::Error as EyreError;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{
    from_str as from_json_str, from_value as from_json_value, to_string as to_json_string,
    to_value as to_json_value, Value,
};
use tokio::spawn;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::time::interval;
use tracing::{debug, error, field, info, info_span, warn, Instrument};
use uuid::Uuid;

mod execute;
mod subscribe;
mod unsubscribe;

pub(crate) use subscribe::{may_observe_context, may_observe_group};

/// Globally unique identifier of a WebSocket client connection. Internal to the
/// server (log correlation + connection-map key); never serialized to clients,
/// so it lives here rather than in `calimero-server-primitives`.
pub(crate) type ConnectionId = Uuid;

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
    /// Group ids this connection observes for `GroupMembership` events. A
    /// distinct id-space from `subscriptions` (context ids), routed
    /// independently; keeping the two sets separate leaves context delivery
    /// byte-for-byte unchanged for existing clients.
    group_subscriptions: HashSet<Hash>,
    last_pong: AtomicU64, // Timestamp of last received pong (or connection start)
    /// The verified public key of the authenticated client that opened this
    /// connection, or `None` when the auth method does not provide a
    /// cryptographic key (e.g. embedded username/password auth). Set once at
    /// upgrade time; immutable for the life of the connection.
    pub(crate) caller: Option<calimero_primitives::identity::PublicKey>,
    /// `true` when the auth layer positively confirmed this connection as the
    /// node owner via a non-key method (e.g. embedded username/password).
    /// Distinguishes the "legitimate NodeOwner" path from "no auth at all"
    /// when `caller` is `None`.
    pub(crate) node_owner: bool,
}

impl ConnectionStateInner {
    fn new(caller: Option<calimero_primitives::identity::PublicKey>, node_owner: bool) -> Self {
        Self {
            subscriptions: HashSet::default(),
            group_subscriptions: HashSet::default(),
            last_pong: AtomicU64::new(unix_timestamp()),
            caller,
            node_owner,
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
    // Used by the `execute` request path (query/mutate over the socket); the
    // event-streaming paths only need `node_client`.
    ctx_client: ContextClient,
    connections: RwLock<HashMap<ConnectionId, ConnectionState>>,
    config: WsConfig,
    /// Whether the auth guard is active on this service's routes. When `false`
    /// the server was intentionally started without auth (no-auth mode).
    pub(crate) auth_enabled: bool,
    /// Spawns the shared node-event fan-out task exactly once, lazily on the
    /// first connection (so a WS service that never sees a client never holds
    /// a broadcast-receiver subscription).
    events_fanout: Once,
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
    ctx_client: ContextClient,
    auth_enabled: bool,
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
        format!("{prefix}{base_path}")
    } else {
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!("WebSocket server listening on {}/ws{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_client,
        ctx_client,
        connections: RwLock::default(),
        config: ws_config,
        auth_enabled,
        events_fanout: Once::new(),
    });

    Some((path, get(ws_handler).layer(Extension(state))))
}

async fn ws_handler(
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    Extension(state): Extension<Arc<ServiceState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    auth_node_owner: Option<Extension<AuthenticatedNodeOwner>>,
) -> impl IntoResponse {
    // Validate WebSocket upgrade request
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => {
            debug!("Invalid WebSocket upgrade request: {}", rejection);
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid WebSocket upgrade request: {rejection}"),
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

    // Determine the caller identity from the auth extensions injected by
    // AuthGuardService:
    //   AuthenticatedKey       → verified public key; stored as Some(pk)
    //   AuthenticatedNodeOwner → non-key auth (e.g. embedded username/password);
    //                            stored as None (NodeOwner path in execute)
    //   neither                → two sub-cases:
    //                             - auth_enabled=true  → guard ran but injected nothing;
    //                               warn loudly (misconfiguration signal)
    //                             - auth_enabled=false → intentional no-auth deployment;
    //                               proceed silently at debug level
    let (caller, node_owner) = match (auth_key, auth_node_owner) {
        (Some(ext), _) => (Some(ext.0 .0), false),
        (None, Some(_)) => (None, true),
        (None, None) => {
            if state.auth_enabled {
                warn!(
                    "No auth extensions present on WebSocket upgrade — auth guard may not be running"
                );
                return StatusCode::UNAUTHORIZED.into_response();
            }
            // Intentional no-auth deployment: treat every connection as
            // node-owner so the no-auth path is positively distinguishable
            // from a misconfigured guard (which returns 401 above).
            info!("No-auth mode: WebSocket upgrade proceeding as NodeOwner — auth is disabled");
            (None, true)
        }
    };
    // Cap inbound message/frame size so a single oversized frame cannot exhaust
    // memory during JSON parsing (the connection is closed instead).
    ws.max_message_size(WS_MAX_MESSAGE_BYTES)
        .max_frame_size(WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, state, caller, node_owner))
        .into_response()
}

async fn handle_socket(
    socket: WebSocket,
    state: Arc<ServiceState>,
    caller: Option<calimero_primitives::identity::PublicKey>,
    node_owner: bool,
) {
    let (commands_sender, commands_receiver) = mpsc::channel(WS_COMMAND_CHANNEL_BUFFER_SIZE);

    // Bounds the number of concurrently-processed text frames for this
    // connection (see WS_MAX_CONCURRENT_MESSAGES).
    let message_limiter = Arc::new(Semaphore::new(WS_MAX_CONCURRENT_MESSAGES));

    // Generate a globally unique connection ID. A UUID (vs a per-process
    // counter) stays unique across node restarts and when logs from multiple
    // nodes are aggregated, so a connection can be traced unambiguously.
    let connection_id = Uuid::new_v4();
    let connection_state = ConnectionState {
        commands: commands_sender.clone(),
        inner: Arc::new(RwLock::new(ConnectionStateInner::new(caller, node_owner))),
    };

    {
        let mut connections = state.connections.write().await;
        let _ = connections.insert(connection_id, connection_state.clone());
    }

    debug!(%connection_id, "Client connection established");

    // One fan-out task serves every connection (spawned lazily here so tests
    // that build a `ServiceState` directly get it too). It holds the only
    // broadcast-receiver subscription and serializes each event once, instead
    // of one subscription + one serialization per connected client.
    state.events_fanout.call_once(|| {
        drop(spawn(fan_out_node_events(Arc::clone(&state))));
    });

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
                // Acquire a permit before spawning. When all permits are in use
                // this awaits — applying backpressure to the read loop instead
                // of spawning unbounded handler tasks under a message flood.
                let permit = match Arc::clone(&message_limiter).acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => break, // semaphore closed — connection shutting down
                };
                let state = Arc::clone(&state);
                drop(spawn(async move {
                    // Held for the lifetime of the handler; released on completion.
                    let _permit = permit;
                    handle_text_message(connection_id, state, message).await;
                }));
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

/// The single node-event fan-out task, shared by every WS connection.
///
/// Spawned at most once per service (lazily, on the first connection) and runs
/// for the rest of the process — `state` keeps the node's event sender alive,
/// so the stream only ends at shutdown. Holding one broadcast-receiver
/// subscription here (instead of one per connection) means each `NodeEvent` is
/// cloned out of the broadcast channel once and JSON-serialized once, with the
/// resulting bytes shared across all subscribed connections via
/// [`Command::SendSerialized`] — previously every connection's own event task
/// re-cloned and re-serialized the same event.
/// Which subscription set a `NodeEvent` is delivered against.
#[derive(Clone, Copy)]
enum EventRoute {
    Context(ContextId),
    Group(Hash),
}

async fn fan_out_node_events(state: Arc<ServiceState>) {
    let events = state.node_client.receive_events();

    let mut events = pin!(events);

    while let Some(event) = events.next().await {
        // Route by id-space: context events by `context_id` (unchanged), group
        // membership events by `group_id`. Each connection is tested against
        // the matching subscription set below.
        let route = match &event {
            NodeEvent::Context(context_event) => EventRoute::Context(context_event.context_id),
            NodeEvent::GroupMembership(membership_event) => {
                EventRoute::Group(membership_event.group_id)
            }
        };

        debug!("Received node event: {:?}", event);

        let body = match to_json_value(event) {
            Ok(v) => ResponseBody::Result(v),
            Err(err) => {
                error!(%err, "Failed to serialize node event");
                ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: None },
                ))
            }
        };

        let response = Response { id: None, body };

        // The pushed response is identical for every subscriber (`id` is always
        // `None`), so serialize it once and share the bytes.
        let message: Arc<str> = match to_json_string(&response) {
            Ok(message) => message.into(),
            Err(err) => {
                error!(%err, "Failed to serialize WsResponse");
                continue;
            }
        };

        // Snapshot the matching subscribers under the read lock, then send
        // without holding it, so neither the connection map nor per-connection
        // state stays locked while command channels drain.
        let mut targets = Vec::new();
        {
            let connections = state.connections.read().await;
            for (connection_id, connection) in &*connections {
                let inner = connection.inner.read().await;
                let matched = match route {
                    EventRoute::Context(context_id) => inner.subscriptions.contains(&context_id),
                    EventRoute::Group(group_id) => inner.group_subscriptions.contains(&group_id),
                };
                if matched {
                    targets.push((*connection_id, connection.commands.clone()));
                }
            }
        }

        for (connection_id, commands) in targets {
            // `try_send` so one slow client's full command channel can't stall
            // event delivery to everyone else; that client just misses this
            // event — the same skip-on-lag contract its dedicated broadcast
            // receiver had before.
            if let Err(err) = commands.try_send(Command::SendSerialized(Arc::clone(&message))) {
                debug!(
                    %connection_id,
                    %err,
                    "Dropping node event for slow or closing connection",
                );
            }
        }
    }

    debug!("Node event stream ended, stopping WS event fan-out");
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
            Command::SendSerialized(message) => {
                // One copy of the shared bytes into the frame's `String` — the
                // serialization itself already happened once, upstream.
                if let Err(err) = socket_sender
                    .send(Message::Text((*message).to_owned()))
                    .await
                {
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
    // One correlation id per inbound message. The server-generated `request_id`
    // is the primary trace key (always present, globally unique); the client's
    // optional `id` is recorded as `client_id` once parsed so a caller who sent
    // one can line their id up with the server trace. `connection_id` ties the
    // message back to its connection. The span propagates through every await
    // below, so downstream logs inherit all three without threading params.
    let request_id = Uuid::new_v4();
    // `context_id`/`method` start empty and are recorded by the `execute`
    // handler once the payload is parsed, so the shared `execute_request`'s
    // downstream logs inherit them — matching the JSON-RPC `rpc_request` span.
    let span = info_span!(
        "ws_request",
        %connection_id,
        %request_id,
        client_id = field::Empty,
        context_id = field::Empty,
        method = field::Empty,
    );

    async move {
        debug!(%message, "Received text message");
        let Some(connection_state) = state.connections.read().await.get(&connection_id).cloned()
        else {
            error!("Connection not found in state map");
            return;
        };

        let message = match from_json_str::<WsRequest<Value>>(&message) {
            Ok(message) => message,
            Err(err) => {
                error!(%err, "Failed to deserialize Request<Value>");
                return;
            }
        };

        if let Some(client_id) = message.id {
            tracing::Span::current().record("client_id", client_id);
        }

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
                RequestPayload::Execute(request) => {
                    let inner = connection_state.inner.read().await;
                    // caller and node_owner are set once at upgrade time and
                    // never mutated; copying them before dropping the lock is safe.
                    let (caller, node_owner) = (inner.caller, inner.node_owner);
                    drop(inner);
                    execute::handle(&state, caller, node_owner, request).await
                }
            },
            Err(err) => {
                error!(%err, "Failed to deserialize RequestPayload");

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
            error!(%err, "Failed to send WsCommand::Send");
        };
    }
    .instrument(span)
    .await;
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
                Err(err) => {
                    error!(%err, "Failed to serialize response");
                    ResponseBody::Error(ResponseBodyError::ServerError(
                        ServerResponseError::InternalError { err: None },
                    ))
                }
            },
            Err(WsError::MethodCallError(err)) => match to_json_value(err) {
                Ok(v) => ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(err) => {
                    error!(%err, "Failed to serialize handler error");
                    ResponseBody::Error(ResponseBodyError::ServerError(
                        ServerResponseError::InternalError { err: None },
                    ))
                }
            },
            Err(WsError::InternalError(err)) => {
                error!(%err, "Internal server error");
                ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: None },
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

use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};
use crate::config::ServerConfig;

/// WebSocket command channel buffer size
///
/// This controls how many WebSocket commands can be queued in the channel before
/// the sender blocks. Should match SSE's COMMAND_CHANNEL_BUFFER_SIZE for consistency.
const WS_COMMAND_CHANNEL_BUFFER_SIZE: usize = 32;

/// Maximum number of text messages from a single connection being processed
/// concurrently. Each text frame was previously `spawn`ed unconditionally, so a
/// client flooding messages could spawn unbounded tasks (memory/CPU exhaustion).
/// A per-connection permit pool bounds in-flight handlers and applies
/// backpressure (the read loop awaits a permit before accepting the next frame).
const WS_MAX_CONCURRENT_MESSAGES: usize = 32;

/// Maximum size of a single inbound WebSocket message. Frames are JSON-parsed in
/// full, so without a cap a single huge frame could exhaust memory. Oversized
/// messages cause the connection to be closed by the WebSocket layer rather than
/// buffered. 16 MiB comfortably covers legitimate execute payloads.
const WS_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    //! Real-socket integration tests for the WebSocket server.
    //!
    //! These bind an ephemeral TCP port, serve the actual `ws_handler` over a
    //! test [`ServiceState`] backed by an in-memory store, and drive it with a
    //! real `tokio-tungstenite` client. They exercise the full upgrade →
    //! message → response path (no router internals are mocked), covering
    //! connection lifecycle, subscribe/unsubscribe, ping/pong, cleanup, event
    //! broadcasting, and the `execute` plumbing.

    use std::sync::Arc;
    use std::time::Duration;

    use axum::routing::get;
    use axum::{Extension, Router};
    use calimero_blobstore::config::BlobStoreConfig;
    use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
    use calimero_context_client::client::ContextClient;
    use calimero_network_primitives::client::NetworkClient;
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::events::{
        ContextEvent, ContextEventPayload, GroupMembershipEvent, MembershipChange,
        MembershipChangePayload, NodeEvent, StateMutationPayload,
    };
    use calimero_primitives::hash::Hash;
    use calimero_primitives::identity::PublicKey;
    use calimero_server_primitives::jsonrpc::ExecutionRequest;
    use calimero_server_primitives::ws::{
        Request as WsRequest, RequestPayload, SubscribeRequest, UnsubscribeRequest,
    };
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use futures_util::{SinkExt, Stream, StreamExt};
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio::sync::{broadcast, mpsc, RwLock};
    use tokio::time::sleep;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};

    use super::{ws_handler, ServiceState, WsConfig};

    /// Everything a test needs to talk to a running WS server: the bound URL,
    /// a handle to the shared state (for asserting on the connection map), and
    /// the event sender (to inject `NodeEvent`s). `_blob_dir` keeps the blob
    /// store's temp dir alive for the duration of the test.
    struct TestServer {
        url: String,
        state: Arc<ServiceState>,
        event_sender: broadcast::Sender<NodeEvent>,
        _blob_dir: TempDir,
        // Kept so the serve task is aborted when the test ends rather than
        // leaking, and so a panic in it isn't silently swallowed by an
        // immediate `drop`.
        _server: tokio::task::JoinHandle<()>,
    }

    async fn spawn_test_ws() -> TestServer {
        spawn_test_ws_full(false, None).await
    }

    // Auth-enabled server whose upgrades carry an authenticated (non-owner)
    // caller, so the per-request subscribe auth gates actually run (an
    // auth-enabled server with no caller extension is rejected at upgrade).
    async fn spawn_test_ws_authed(caller: PublicKey) -> TestServer {
        spawn_test_ws_full(true, Some(caller)).await
    }

    async fn spawn_test_ws_full(auth_enabled: bool, caller: Option<PublicKey>) -> TestServer {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        let blob_dir = TempDir::new().unwrap();
        let blob_store = BlobStore::new(
            store.clone(),
            FileSystem::new(&BlobStoreConfig::new(
                blob_dir.path().to_path_buf().try_into().unwrap(),
            ))
            .await
            .unwrap(),
        );
        let blob_manager = BlobManager::new(blob_store);

        // Initial receiver dropped immediately so `receiver_count()` reflects
        // only the shared node-event fan-out task.
        let (event_sender, _) = broadcast::channel(256);
        let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(64);
        let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(64);
        let (ns_join_tx, _ns_join_rx) = mpsc::channel(16);
        let (open_subgroup_join_tx, _open_subgroup_join_rx) = mpsc::channel(16);
        let sync_client =
            SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

        let node_client = NodeClient::new(
            store.clone(),
            blob_manager,
            NetworkClient::new(LazyRecipient::new()),
            LazyRecipient::new(),
            event_sender.clone(),
            sync_client,
            None,
        );
        let ctx_client = ContextClient::new(store, node_client.clone(), LazyRecipient::new());

        let state = Arc::new(ServiceState {
            node_client,
            ctx_client,
            connections: RwLock::default(),
            config: WsConfig::new(true),
            auth_enabled,
            events_fanout: std::sync::Once::new(),
        });

        let mut app = Router::new().route("/ws", get(ws_handler));
        if let Some(caller) = caller {
            app = app.layer(Extension(crate::auth::AuthenticatedKey(caller)));
        }
        let app = app.layer(Extension(Arc::clone(&state)));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        TestServer {
            url: format!("ws://{addr}/ws"),
            state,
            event_sender,
            _server: server,
            _blob_dir: blob_dir,
        }
    }

    /// Read frames until a text frame arrives (skipping ping/pong), parsed as
    /// JSON. Returns `None` if `dur` elapses or the stream ends first.
    async fn next_json<S>(read: &mut S, dur: Duration) -> Option<Value>
    where
        S: Stream<Item = Result<Message, WsError>> + Unpin,
    {
        tokio::time::timeout(dur, async {
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(text) = msg {
                    return Some(serde_json::from_str(&text).expect("server sent valid json"));
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }

    /// Poll the connection map until it reaches `want` entries or 5s elapses.
    async fn wait_conn_count(state: &ServiceState, want: usize) -> bool {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state.connections.read().await.len() == want {
                    return;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok()
    }

    fn subscribe_msg(id: u64, ctx: ContextId) -> Message {
        let req = WsRequest {
            id: Some(id),
            payload: RequestPayload::Subscribe(SubscribeRequest {
                context_ids: vec![ctx],
                group_ids: vec![],
            }),
        };
        Message::Text(serde_json::to_string(&req).unwrap())
    }

    fn subscribe_group_msg(id: u64, group: Hash) -> Message {
        let req = WsRequest {
            id: Some(id),
            payload: RequestPayload::Subscribe(SubscribeRequest {
                context_ids: vec![],
                group_ids: vec![group],
            }),
        };
        Message::Text(serde_json::to_string(&req).unwrap())
    }

    fn group_membership_event(group: Hash) -> NodeEvent {
        NodeEvent::GroupMembership(GroupMembershipEvent {
            group_id: group,
            payload: MembershipChangePayload::MemberJoined(MembershipChange {
                member: PublicKey::from([9u8; 32]),
                role: None,
            }),
        })
    }

    // A group subscriber receives GroupMembership events for its group; a
    // connection that did not subscribe the group id receives nothing. Mirrors
    // `events_only_reach_subscribers` for the group id-space (auth disabled, so
    // the subscription itself is always admitted).
    #[tokio::test]
    async fn group_membership_events_only_reach_group_subscribers() {
        let server = spawn_test_ws().await;
        let group = Hash::from([77u8; 32]);

        let (mut write_a, mut read_a) = connect_async(&server.url).await.unwrap().0.split();
        let (_write_b, mut read_b) = connect_async(&server.url).await.unwrap().0.split();

        write_a.send(subscribe_group_msg(1, group)).await.unwrap();
        let sub_resp = next_json(&mut read_a, Duration::from_secs(5))
            .await
            .expect("subscribe response");
        assert_eq!(sub_resp["id"], json!(1));
        assert_eq!(
            sub_resp["result"]["groupIds"],
            serde_json::to_value(vec![group]).unwrap(),
            "the group id should be echoed as subscribed"
        );

        let listening = tokio::time::timeout(Duration::from_secs(5), async {
            while server.event_sender.receiver_count() < 1 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        assert!(listening, "the event fan-out task should be listening");

        server
            .event_sender
            .send(group_membership_event(group))
            .unwrap();

        let pushed = next_json(&mut read_a, Duration::from_secs(5))
            .await
            .expect("group subscriber should receive the event");
        assert_eq!(
            pushed["result"]["type"], "MemberJoined",
            "the frame should carry the membership-change discriminant: {pushed}"
        );
        assert!(
            pushed["result"].get("groupId").is_some(),
            "the frame should carry the groupId: {pushed}"
        );

        let leaked = next_json(&mut read_b, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "a non-group-subscriber must not receive the event: {leaked:?}"
        );
    }

    // With auth enabled and an authenticated caller that is NOT a member of the
    // group, `may_observe_group` denies the subscription (the response lists no
    // group ids) and no event is delivered - proving the group-scoped auth gate
    // is wired and fails closed against a non-member. The empty in-memory store
    // makes `is_member` false for any group.
    #[tokio::test]
    async fn group_subscribe_denied_for_non_member() {
        let non_member = PublicKey::from([0x55u8; 32]);
        let server = spawn_test_ws_authed(non_member).await;
        let group = Hash::from([88u8; 32]);

        let (mut write, mut read) = connect_async(&server.url).await.unwrap().0.split();
        write.send(subscribe_group_msg(1, group)).await.unwrap();
        let resp = next_json(&mut read, Duration::from_secs(5))
            .await
            .expect("subscribe response");
        assert_eq!(resp["id"], json!(1));
        let denied = resp["result"]
            .get("groupIds")
            .and_then(|v| v.as_array())
            .is_none_or(|a| a.is_empty());
        assert!(
            denied,
            "non-member group subscription must be denied: {resp}"
        );

        let listening = tokio::time::timeout(Duration::from_secs(5), async {
            while server.event_sender.receiver_count() < 1 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        assert!(listening, "the event fan-out task should be listening");

        server
            .event_sender
            .send(group_membership_event(group))
            .unwrap();

        let leaked = next_json(&mut read, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "a denied subscriber must not receive the group event: {leaked:?}"
        );
    }

    // The important auth edge (design risk #1): a member kicked from an Open
    // subgroup keeps an inheritance path (kick = deny-list entry, not row
    // deletion), so the deny-list-BLIND `is_member`/`check_path` would still
    // pass and leak them the subgroup's events. The gate uses the deny-list-
    // AWARE `effective_capabilities`, so the deny-listed inherited member is
    // denied the subscription and receives no event - matching the member set
    // `list_group_members` exposes.
    #[tokio::test]
    async fn group_subscribe_denied_for_deny_listed_inherited_member() {
        use calimero_context::group_store::{
            CapabilitiesRepository, DenyListRepository, MembershipRepository, NamespaceRepository,
        };
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::{MemberCapabilities, VisibilityMode};
        use calimero_primitives::context::GroupMemberRole;

        let bob_sk = calimero_primitives::identity::PrivateKey::random(&mut rand::rngs::OsRng);
        let bob_pk = bob_sk.public_key();
        let server = spawn_test_ws_authed(bob_pk).await;

        let ns_gid = ContextGroupId::from([0xB0u8; 32]);
        let subgroup = ContextGroupId::from([0xB1u8; 32]);
        let store = server.state.ctx_client.datastore();

        // Bob is a namespace member holding CAN_JOIN_OPEN_SUBGROUPS, the subgroup
        // is Open and nested under the namespace, and Bob has NO direct subgroup
        // row - so he is an inherited member of the subgroup.
        MembershipRepository::new(store)
            .add_member(&ns_gid, &bob_pk, GroupMemberRole::Member)
            .unwrap();
        CapabilitiesRepository::new(store)
            .set_member_capability(
                &ns_gid,
                &bob_pk,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .unwrap();
        NamespaceRepository::new(store)
            .nest(&ns_gid, &subgroup)
            .unwrap();
        CapabilitiesRepository::new(store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .unwrap();

        // The kick: a per-subgroup deny-list entry. `is_member` (deny-list-blind)
        // still passes; `effective_capabilities` (deny-list-aware) does not.
        DenyListRepository::new(store)
            .mark(&subgroup, &bob_pk)
            .unwrap();
        assert!(
            MembershipRepository::new(store)
                .is_member(&subgroup, &bob_pk)
                .unwrap(),
            "precondition: the deny-list-blind check still sees Bob as a member (that is the bug)"
        );
        assert!(
            MembershipRepository::new(store)
                .effective_capabilities(&subgroup, &bob_pk)
                .unwrap()
                .is_none(),
            "precondition: the deny-list-aware check must exclude the kicked member"
        );

        let group = Hash::from(subgroup.to_bytes());
        let (mut write, mut read) = connect_async(&server.url).await.unwrap().0.split();
        write.send(subscribe_group_msg(1, group)).await.unwrap();
        let resp = next_json(&mut read, Duration::from_secs(5))
            .await
            .expect("subscribe response");
        let denied = resp["result"]
            .get("groupIds")
            .and_then(|v| v.as_array())
            .is_none_or(|a| a.is_empty());
        assert!(
            denied,
            "a deny-listed inherited member must be denied the group subscription: {resp}"
        );

        let listening = tokio::time::timeout(Duration::from_secs(5), async {
            while server.event_sender.receiver_count() < 1 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        assert!(listening, "the event fan-out task should be listening");

        server
            .event_sender
            .send(group_membership_event(group))
            .unwrap();

        let leaked = next_json(&mut read, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "a deny-listed inherited member must not receive the group event: {leaked:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_connections_get_distinct_entries() {
        let server = spawn_test_ws().await;

        // Open several connections; each must register a distinct entry in the
        // connection map. Distinct entries (len == N) prove unique ids, since
        // the map is keyed by connection id and a collision would drop one.
        let mut conns = Vec::new();
        for _ in 0..5 {
            conns.push(connect_async(&server.url).await.unwrap().0);
        }

        assert!(
            wait_conn_count(&server.state, 5).await,
            "all 5 connections should register distinct entries"
        );
    }

    #[tokio::test]
    async fn connection_cleanup_on_disconnect() {
        let server = spawn_test_ws().await;

        let conn = connect_async(&server.url).await.unwrap().0;
        assert!(wait_conn_count(&server.state, 1).await);

        // Closing the client should make the server drop the connection entry.
        drop(conn);
        assert!(
            wait_conn_count(&server.state, 0).await,
            "connection entry should be removed after disconnect"
        );
    }

    #[tokio::test]
    async fn subscribe_and_unsubscribe_round_trip() {
        let server = spawn_test_ws().await;
        let (mut write, mut read) = connect_async(&server.url).await.unwrap().0.split();

        let ctx = ContextId::from([7u8; 32]);

        write.send(subscribe_msg(1, ctx)).await.unwrap();
        let resp = next_json(&mut read, Duration::from_secs(5))
            .await
            .expect("subscribe response");
        assert_eq!(resp["id"], json!(1));
        assert_eq!(
            resp["result"]["contextIds"],
            serde_json::to_value(vec![ctx]).unwrap()
        );

        let unsub = WsRequest {
            id: Some(2),
            payload: RequestPayload::Unsubscribe(UnsubscribeRequest {
                context_ids: vec![ctx],
                group_ids: vec![],
            }),
        };
        write
            .send(Message::Text(serde_json::to_string(&unsub).unwrap()))
            .await
            .unwrap();
        let resp = next_json(&mut read, Duration::from_secs(5))
            .await
            .expect("unsubscribe response");
        assert_eq!(resp["id"], json!(2));
        assert_eq!(
            resp["result"]["contextIds"],
            serde_json::to_value(vec![ctx]).unwrap()
        );
    }

    #[tokio::test]
    async fn ping_gets_pong() {
        let server = spawn_test_ws().await;
        let (mut write, mut read) = connect_async(&server.url).await.unwrap().0.split();

        write.send(Message::Ping(vec![1, 2, 3])).await.unwrap();

        let got_pong = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(Ok(msg)) = read.next().await {
                if matches!(msg, Message::Pong(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);

        assert!(got_pong, "server should answer a ping with a pong");
    }

    #[tokio::test]
    async fn events_only_reach_subscribers() {
        let server = spawn_test_ws().await;
        let ctx = ContextId::from([42u8; 32]);

        // A subscribes to ctx; B stays unsubscribed.
        let (mut write_a, mut read_a) = connect_async(&server.url).await.unwrap().0.split();
        let (_write_b, mut read_b) = connect_async(&server.url).await.unwrap().0.split();

        write_a.send(subscribe_msg(1, ctx)).await.unwrap();
        let sub_resp = next_json(&mut read_a, Duration::from_secs(5))
            .await
            .expect("subscribe response");
        assert_eq!(sub_resp["id"], json!(1));

        // Wait until the shared fan-out task (spawned on the first connection)
        // has subscribed to the broadcast channel, otherwise the injected event
        // could be sent before a receiver exists and be lost.
        let listening = tokio::time::timeout(Duration::from_secs(5), async {
            while server.event_sender.receiver_count() < 1 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        assert!(listening, "the event fan-out task should be listening");

        let event = NodeEvent::Context(ContextEvent {
            context_id: ctx,
            payload: ContextEventPayload::StateMutation(
                StateMutationPayload::with_root_and_events(Hash::default(), vec![]),
            ),
        });
        let _ = server.event_sender.send(event).unwrap();

        // A (subscribed) receives the event...
        let pushed = next_json(&mut read_a, Duration::from_secs(5))
            .await
            .expect("subscriber should receive the event");
        assert!(
            pushed.get("result").is_some(),
            "event push should carry a result body: {pushed}"
        );

        // ...B (not subscribed) receives nothing.
        let leaked = next_json(&mut read_b, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "non-subscriber must not receive the event: {leaked:?}"
        );
    }

    #[tokio::test]
    async fn execute_for_unknown_context_returns_error() {
        let server = spawn_test_ws().await;
        let (mut write, mut read) = connect_async(&server.url).await.unwrap().0.split();

        // No context exists in the in-memory store, so executor resolution fails
        // before the runtime is ever invoked. This exercises the full
        // parse → validate → execute_request → error-mapping path over the socket
        // (a live happy-path execute needs a running runtime and is covered by
        // the e2e suite).
        let req = WsRequest {
            id: Some(7),
            payload: RequestPayload::Execute(ExecutionRequest::new(
                ContextId::from([3u8; 32]),
                "some_method".to_owned(),
                json!({}),
                vec![],
            )),
        };
        write
            .send(Message::Text(serde_json::to_string(&req).unwrap()))
            .await
            .unwrap();

        let resp = next_json(&mut read, Duration::from_secs(5))
            .await
            .expect("execute response");
        assert_eq!(resp["id"], json!(7));
        assert!(
            resp.get("error").is_some(),
            "execute against a non-existent context should error: {resp}"
        );
    }
}
