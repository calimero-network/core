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

use super::config::{retry_timeout, SESSION_EXPIRY_SECS};
use super::events::{handle_connection_cleanup, handle_node_events};
use super::session::{now_secs, ActiveConnection, SessionState, SessionStateInner};
use super::state::ServiceState;
use super::storage::{delete_session, load_session, save_session};

/// Handle subscription/unsubscription requests
pub async fn handle_subscription(
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
                let mut invalid = Vec::new();

                for ctx in &ctxs.context_ids {
                    if !inner.subscriptions.remove(ctx) {
                        invalid.push(*ctx);
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

/// Handle SSE connection establishment
#[expect(
    clippy::too_many_lines,
    reason = "Complex handler with multiple reconnection paths"
)]
pub async fn sse_handler(
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
