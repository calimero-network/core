//! SSE (Server-Sent Events) implementation for real-time event streaming
//!
//! # Architecture Overview
//!
//! This module implements a session-based SSE system with the following components:
//! - **Sessions**: Persistent client sessions with unique IDs, subscriptions, and event counters
//! - **Connections**: Ephemeral HTTP/SSE connections that can disconnect and reconnect
//! - **Events**: Node events filtered by subscription and delivered over active connections
//!
//! # Event Delivery Model: Skip-on-Disconnect
//!
//! This implementation uses a **skip-on-disconnect** approach:
//! - ✅ Sessions persist across reconnections (subscriptions, event counter, etc.)
//! - ✅ Event IDs are sequential and monotonically increasing per session
//! - ❌ Events are **NOT buffered** - they only go to active connections
//! - ❌ Events occurring during disconnection are **permanently skipped**
//!
//! When clients reconnect:
//! 1. Session state is restored (subscriptions, counter position)
//! 2. New events continue from the current counter value
//! 3. Event ID gaps indicate missed events during disconnection
//! 4. Clients should re-query application state to handle gaps
//!
//! # Design Rationale
//!
//! This design prioritizes:
//! - **Simplicity**: No complex buffering or replay logic
//! - **Resource efficiency**: No memory overhead for buffering events
//! - **Scalability**: Constant memory usage per session
//!
//! Trade-offs:
//! - Clients must handle missed events via state reconciliation
//! - Not suitable for guaranteed delivery use cases
//! - Best for real-time notifications where missing some is acceptable

use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Request, RequestPayload, Response, ResponseBody, ResponseBodyError,
    ServerResponseError, SseEvent,
};
use core::convert::Infallible;
use futures_util::stream::{self as stream, Stream};
use futures_util::StreamExt;
use rand::random;
use serde_json::to_string as to_json_string;
use std::collections::hash_map::Entry;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, warn};

use super::config::{retry_timeout, COMMAND_CHANNEL_BUFFER_SIZE, SESSION_EXPIRY_SECS};
use super::events::{handle_connection_cleanup, handle_node_events};
use super::session::{now_secs, ActiveConnection, SessionState, SessionStateInner};
use super::state::ServiceState;
use super::storage::{delete_session, load_session, save_session};

/// Handle subscription/unsubscription requests
pub async fn handle_subscription(
    Extension(state): Extension<Arc<ServiceState>>,
    Json(request): Json<Request<serde_json::Value>>,
) -> impl IntoResponse {
    let session_id = match request.id.parse::<ConnectionId>() {
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
                inner.touch();

                // Persist to store
                let persisted = inner.to_persisted();
                drop(inner);
                drop(sessions);

                let mut store = state.store.clone();
                if let Err(err) = save_session(&mut store, session_id, &persisted) {
                    error!(%session_id, %err, "Failed to persist session subscriptions");
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
                let mut unsubscribed = Vec::new();

                // Remove contexts that were actually subscribed
                // This is an idempotent operation - attempting to unsubscribe from
                // a context that wasn't subscribed is not an error
                for ctx in &ctxs.context_ids {
                    if inner.subscriptions.remove(ctx) {
                        unsubscribed.push(*ctx);
                    }
                }
                inner.touch();

                // Persist to store
                let persisted = inner.to_persisted();
                drop(inner);
                drop(sessions);

                let mut store = state.store.clone();
                if let Err(err) = save_session(&mut store, session_id, &persisted) {
                    error!(%session_id, %err, "Failed to persist session after unsubscribe");
                }

                // Idempotent operation - always return OK with info about what was unsubscribed
                // Response includes:
                // - "unsubscribed": contexts that were actually removed from subscriptions
                // - "requested": contexts that the client requested to unsubscribe from
                // Clients can compare these to detect contexts they weren't subscribed to
                (
                    StatusCode::OK,
                    Json(Response {
                        body: ResponseBody::Result(serde_json::json!({
                            "status": "unsubscribed",
                            "unsubscribed": unsubscribed,
                            "requested": ctxs.context_ids,
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

/// Handle SSE connection establishment
#[expect(
    clippy::too_many_lines,
    reason = "Complex handler with multiple reconnection paths"
)]
/// Handle SSE stream connections and reconnections
///
/// # Reconnection Behavior
///
/// This handler supports session-based reconnection using the `Last-Event-ID` header:
/// - New clients get a new session with a fresh event counter starting at 0
/// - Reconnecting clients provide their last event ID (format: `{session_id}-{event_num}`)
/// - Sessions persist for up to [`SESSION_EXPIRY_SECS`] seconds across reconnections
///
/// **Important**: While sessions persist, **events are NOT buffered**. When a client
/// reconnects, they will:
/// - Resume their session with the same session ID and subscriptions
/// - Continue receiving new events from the current counter value
/// - **NOT** receive events that occurred during disconnection (these are skipped)
/// Clients observing gaps in event IDs should re-query application state as needed.
pub async fn sse_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    request: AxumRequest,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let headers = request.headers();

    // Check for Last-Event-ID header for reconnection
    // Format: "{session_id}-{event_number}"
    // We extract the session_id to restore subscriptions and counter position
    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split('-').next())
        .and_then(|id| id.parse::<ConnectionId>().ok());

    let (commands_sender, commands_receiver) = mpsc::channel::<Command>(COMMAND_CHANNEL_BUFFER_SIZE);

    let (session_id, session_state, is_reconnect) = if let Some(existing_session_id) = last_event_id
    {
        // Attempt to reconnect to existing session
        let sessions = state.sessions.read().await;
        if let Some(existing_session) = sessions.get(&existing_session_id).cloned() {
            // Check expiry
            if existing_session.inner.read().await.is_expired() {
                drop(sessions);
                warn!(%existing_session_id, "Session expired, creating new session");
                create_new_session(&state).await
            } else {
                info!(%existing_session_id, "Client reconnecting to existing session (from cache)");
                (existing_session_id, existing_session, true)
            }
        } else {
            drop(sessions);
            // Try to load from persistent storage
            match load_session(&state.store, existing_session_id) {
                Ok(Some(persisted_data)) => {
                    // Check if session expired
                    if now_secs() - persisted_data.last_activity > SESSION_EXPIRY_SECS {
                        warn!(%existing_session_id, "Persisted session expired, creating new session");
                        // Clean up expired session
                        let mut store = state.store.clone();
                        drop(delete_session(&mut store, existing_session_id));
                        create_new_session(&state).await
                    } else {
                        info!(%existing_session_id, "Client reconnecting to persisted session");
                        // Restore session from storage
                        let session_state = SessionState {
                            inner: Arc::new(RwLock::new(SessionStateInner::from_persisted(
                                persisted_data,
                            ))),
                        };
                        // Add to in-memory cache
                        drop(
                            state
                                .sessions
                                .write()
                                .await
                                .insert(existing_session_id, session_state.clone()),
                        );
                        (existing_session_id, session_state, true)
                    }
                }
                Ok(None) => {
                    warn!(%existing_session_id, "Session not found in storage, creating new session");
                    create_new_session(&state).await
                }
                Err(err) => {
                    error!(%existing_session_id, %err, "Failed to load session from storage, creating new session");
                    create_new_session(&state).await
                }
            }
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
    drop(tokio::spawn(handle_connection_cleanup(
        session_id,
        Arc::clone(&state),
        commands_sender.clone(),
    )));

    // Convert commands to SSE events with event IDs
    let event_counter = Arc::clone(&session_state.inner);
    let command_stream = ReceiverStream::new(commands_receiver).map(move |command| {
        let event_id = event_counter
            .blocking_read()
            .event_counter
            .fetch_add(1, Ordering::SeqCst);
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
        .retry(retry_timeout())
        .data(&session_id.to_string());
    let initial_stream = stream::once(async { Ok(initial_event) });

    let stream = initial_stream.chain(command_stream);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Create a new session with persistent storage
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

                // Persist new session to store
                let persisted = session_state.inner.blocking_read().to_persisted();
                drop(sessions);

                let mut store = state.store.clone();
                if let Err(err) = save_session(&mut store, session_id, &persisted) {
                    error!(%session_id, %err, "Failed to persist new session to storage");
                }

                return (session_id, session_state, false);
            }
        }
    }
}
