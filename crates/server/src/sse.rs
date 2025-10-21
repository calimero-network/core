use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Extension;
use axum::Json;
use axum::Router;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Request, RequestPayload, Response, ResponseBody, ResponseBodyError,
    ServerResponseError, SseEvent,
};
use core::convert::Infallible;
use core::pin::pin;
use core::time::Duration;
use futures_util::stream::{self as stream, Stream};
use futures_util::StreamExt;
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::{to_string as to_json_string, to_value as to_json_value};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, warn};

use crate::config::ServerConfig;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SseConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

impl SseConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

// Retry timeout for client reconnection (in milliseconds)
const SSE_RETRY_TIMEOUT_MS: u64 = 3000;

#[derive(Debug)]
pub(crate) struct SessionStateInner {
    subscriptions: HashSet<ContextId>,
    event_counter: AtomicU64,
}

impl Default for SessionStateInner {
    fn default() -> Self {
        Self {
            subscriptions: HashSet::new(),
            event_counter: AtomicU64::new(0),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SessionState {
    inner: Arc<RwLock<SessionStateInner>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveConnection {
    commands: mpsc::Sender<Command>,
}

pub(crate) struct ServiceState {
    node_client: NodeClient,
    // Session state persists across reconnections
    sessions: RwLock<HashMap<ConnectionId, SessionState>>,
    // Active connections track current SSE streams
    active_connections: RwLock<HashMap<ConnectionId, ActiveConnection>>,
}

pub(crate) fn service(
    config: &ServerConfig,
    node_client: NodeClient,
) -> Option<(&'static str, Router)> {
    let _ = match &config.sse {
        Some(config) if config.enabled => config,
        _ => {
            info!("SSE server is disabled");
            return None;
        }
    };

    let path = "/sse";

    for listen in &config.listen {
        info!("SSE server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_client,
        sessions: RwLock::default(),
        active_connections: RwLock::default(),
    });
    let router = Router::new()
        .route("/", get(sse_handler))
        .route("/subscription", post(handle_subscription))
        .layer(Extension(state));

    Some((path, router))
}

async fn handle_subscription(
    Extension(state): Extension<Arc<ServiceState>>,
    Json(request): Json<Request<serde_json::Value>>,
) -> impl IntoResponse {
    let session_id = match request.id.parse::<u64>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(Response {
                    body: ResponseBody::Error(ResponseBodyError::HandlerError(
                        "Invalid Session Id".into(),
                    )),
                }),
            );
        }
    };

    match serde_json::from_value(request.payload) {
        Ok(RequestPayload::Subscribe(ctxs)) => {
            info!(
                "Subscribe: session_id = {:?}, context_ids = {:?}",
                session_id, ctxs
            );

            let sessions = state.sessions.read().await;

            if let Some(session) = sessions.get(&session_id) {
                let mut inner = session.inner.write().await;
                for ctx in &ctxs.context_ids {
                    let _ = inner.subscriptions.insert(*ctx);
                }

                (
                    StatusCode::OK,
                    Json(Response {
                        body: ResponseBody::Result(serde_json::json!({
                            "status": "subscribed",
                            "contexts": ctxs.context_ids,
                        })),
                    }),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(Response {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Session not found. Please reconnect to SSE endpoint first.".into(),
                        )),
                    }),
                )
            }
        }
        Ok(RequestPayload::Unsubscribe(ctxs)) => {
            info!(
                "Unsubscribe: session_id = {:?}, context_ids = {:?}",
                session_id, ctxs
            );

            let sessions = state.sessions.read().await;
            if let Some(session) = sessions.get(&session_id) {
                let mut inner = session.inner.write().await;
                let mut invalid = Vec::new();

                for ctx in &ctxs.context_ids {
                    if !inner.subscriptions.remove(ctx) {
                        invalid.push(*ctx);
                    }
                }

                if !invalid.is_empty() {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(Response {
                            body: ResponseBody::Error(ResponseBodyError::HandlerError(
                                "Some context IDs were not subscribed".into(),
                            )),
                        }),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(Response {
                            body: ResponseBody::Result(serde_json::json!({
                                "status": "unsubscribed",
                                "contexts": ctxs.context_ids,
                            })),
                        }),
                    )
                }
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(Response {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Session not found. Please reconnect to SSE endpoint first.".into(),
                        )),
                    }),
                )
            }
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(Response {
                body: ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::ParseError(err.to_string()),
                )),
            }),
        ),
    }
}

async fn sse_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    request: AxumRequest,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let headers = request.headers();
    
    // Check for Last-Event-ID header for reconnection
    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split('-').next())
        .and_then(|id| id.parse::<u64>().ok());

    let (commands_sender, commands_receiver) = mpsc::channel::<Command>(32);

    let (session_id, session_state, is_reconnect) = if let Some(existing_session_id) = last_event_id {
        // Attempt to reconnect to existing session
        let sessions = state.sessions.read().await;
        if let Some(existing_session) = sessions.get(&existing_session_id).cloned() {
            info!(%existing_session_id, "Client reconnecting to existing session");
            (existing_session_id, existing_session, true)
        } else {
            // Session expired or doesn't exist, create new one
            drop(sessions);
            warn!(%existing_session_id, "Session not found for reconnection, creating new session");
            create_new_session(&state).await
        }
    } else {
        // New connection, create new session
        create_new_session(&state).await
    };

    // Register active connection
    let mut active_connections = state.active_connections.write().await;
    drop(active_connections.insert(
        session_id,
        ActiveConnection {
            commands: commands_sender.clone(),
        },
    ));
    drop(active_connections);

    if is_reconnect {
        info!(%session_id, "Client reconnected, subscriptions restored");
    } else {
        debug!(%session_id, "New client session established");
    }

    // Spawn event handler
    drop(tokio::spawn(handle_node_events(
        session_id,
        Arc::clone(&state),
        session_state.clone(),
    )));

    // Spawn cleanup handler
    drop(tokio::spawn(handle_connection_cleanups(
        session_id,
        Arc::clone(&state),
        commands_sender.clone(),
    )));

    // Convert commands to SSE events with event IDs
    let event_counter = Arc::clone(&session_state.inner);
    let command_stream = ReceiverStream::new(commands_receiver).map(move |command| {
        let event_id = event_counter.blocking_read().event_counter.fetch_add(1, Ordering::SeqCst);
        let id_str = format!("{}-{}", session_id, event_id);
        
        match command {
            Command::Close(reason) => Ok(Event::default()
                .event(SseEvent::Close.as_str())
                .id(id_str)
                .data(reason)),
            Command::Send(response) => match to_json_string(&response) {
                Ok(message) => Ok(Event::default()
                    .event(SseEvent::Message.as_str())
                    .id(id_str)
                    .data(message)),
                Err(err) => {
                    error!("Failed to serialize SseResponse: {}", err);
                    Ok(Event::default()
                        .event(SseEvent::Error.as_str())
                        .id(id_str)
                        .data("Failed to serialize SseResponse"))
                }
            },
        }
    });

    // Initial connection event with retry configuration
    let initial_event = Event::default()
        .event(SseEvent::Connect.as_str())
        .id(format!("{}-0", session_id))
        .retry(Duration::from_millis(SSE_RETRY_TIMEOUT_MS))
        .data(&session_id.to_string());
    let initial_stream = stream::once(async { Ok(initial_event) });

    let stream = initial_stream.chain(command_stream);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn create_new_session(state: &ServiceState) -> (ConnectionId, SessionState, bool) {
    loop {
        let session_id = random();
        let mut sessions = state.sessions.write().await;
        match sessions.entry(session_id) {
            Entry::Occupied(_) => continue,
            Entry::Vacant(entry) => {
                let session_state = SessionState {
                    inner: Arc::new(RwLock::new(SessionStateInner::default())),
                };
                let _ = entry.insert(session_state.clone());
                return (session_id, session_state, false);
            }
        }
    }
}

async fn handle_node_events(
    session_id: ConnectionId,
    state: Arc<ServiceState>,
    session_state: SessionState,
) {
    let events = state.node_client.receive_events();

    let mut events = pin!(events);

    while let Some(event) = events.next().await {
        // Check if there's an active connection for this session
        let active_connections = state.active_connections.read().await;
        let Some(active_conn) = active_connections.get(&session_id).cloned() else {
            debug!(%session_id, "No active connection, waiting for reconnection");
            drop(active_connections);
            
            // Wait a bit before checking again (connection might be reconnecting)
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        drop(active_connections);

        let subscriptions = session_state.inner.read().await.subscriptions.clone();

        debug!(
            %session_id,
            "Received node event: {:?}, subscriptions state: {:?}",
            event,
            subscriptions
        );

        let event = match event {
            NodeEvent::Context(event) if subscriptions.contains(&event.context_id) => {
                NodeEvent::Context(event)
            }
            NodeEvent::Context(_) => continue,
        };

        // Increment event counter (unused return value is intentional)
        let _ = session_state.inner.read().await.event_counter.fetch_add(1, Ordering::SeqCst);

        let body = match to_json_value(event) {
            Ok(v) => ResponseBody::Result(v),
            Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::InternalError {
                    err: Some(err.into()),
                },
            )),
        };

        let response = Response { body };

        if let Err(err) = active_conn.commands.send(Command::Send(response)).await {
            debug!(
                %session_id,
                %err,
                "Failed to send event (connection likely closed, will retry on reconnect)",
            );
            // Don't break - session persists, connection might reconnect
        };
    }
}

async fn handle_connection_cleanups(
    session_id: ConnectionId,
    state: Arc<ServiceState>,
    command_sender: mpsc::Sender<Command>,
) {
    command_sender.closed().await;
    
    // Remove active connection but keep session for reconnection
    drop(state.active_connections.write().await.remove(&session_id));
    
    debug!(%session_id, "Active SSE connection closed (session persists for reconnection)");
}

