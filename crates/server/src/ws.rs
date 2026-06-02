use std::borrow::Cow;
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
use calimero_context_client::client::ContextClient;
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
use serde::{Deserialize, Serialize};
use serde_json::{
    from_str as from_json_str, from_value as from_json_value, to_string as to_json_string,
    to_value as to_json_value, Value,
};
use tokio::spawn;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tracing::{debug, error, field, info, info_span, warn, Instrument};
use uuid::Uuid;

mod execute;
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
    // Used by the `execute` request path (query/mutate over the socket); the
    // event-streaming paths only need `node_client`.
    ctx_client: ContextClient,
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
    ctx_client: ContextClient,
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
        ctx_client,
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

    // Generate a globally unique connection ID. A UUID (vs a per-process
    // counter) stays unique across node restarts and when logs from multiple
    // nodes are aggregated, so a connection can be traced unambiguously.
    let connection_id = Uuid::new_v4();
    let connection_state = ConnectionState {
        commands: commands_sender.clone(),
        inner: Arc::default(),
    };

    {
        let mut connections = state.connections.write().await;
        let _ = connections.insert(connection_id, connection_state.clone());
    }

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
    // One correlation id per inbound message. The server-generated `request_id`
    // is the primary trace key (always present, globally unique); the client's
    // optional `id` is recorded as `client_id` once parsed so a caller who sent
    // one can line their id up with the server trace. `connection_id` ties the
    // message back to its connection. The span propagates through every await
    // below, so downstream logs inherit all three without threading params.
    let request_id = Uuid::new_v4();
    let span = info_span!(
        "ws_request",
        %connection_id,
        %request_id,
        client_id = field::Empty,
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
                RequestPayload::Execute(request) => execute::handle(&state, request).await,
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
        ContextEvent, ContextEventPayload, NodeEvent, StateMutationPayload,
    };
    use calimero_primitives::hash::Hash;
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
    }

    async fn spawn_test_ws() -> TestServer {
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
        // only the per-connection node-event tasks.
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
            String::new(),
            None,
        );
        let ctx_client = ContextClient::new(store, node_client.clone(), LazyRecipient::new());

        let state = Arc::new(ServiceState {
            node_client,
            ctx_client,
            connections: RwLock::default(),
            config: WsConfig::new(true),
        });

        let app = Router::new()
            .route("/ws", get(ws_handler))
            .layer(Extension(Arc::clone(&state)));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));

        TestServer {
            url: format!("ws://{addr}/ws"),
            state,
            event_sender,
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
            }),
        };
        Message::Text(serde_json::to_string(&req).unwrap())
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

        // Wait until both per-connection node-event tasks have subscribed to the
        // broadcast channel, otherwise the injected event could be sent before a
        // receiver exists and be lost.
        let both_listening = tokio::time::timeout(Duration::from_secs(5), async {
            while server.event_sender.receiver_count() < 2 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        assert!(both_listening, "both node-event tasks should be listening");

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
