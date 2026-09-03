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

use axum::extract::{Path, Request as AxumRequest};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::Extension;
use axum::Json;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Request, RequestPayload, Response as SseResponse, ResponseBody,
    ResponseBodyError, ServerResponseError, SseEvent,
};
use core::convert::Infallible;
use futures_util::stream;
use futures_util::StreamExt;
use rand::random;
use serde_json::to_string as to_json_string;
use std::collections::hash_map::Entry;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, warn};

use super::config::{retry_timeout, COMMAND_CHANNEL_BUFFER_SIZE, SESSION_EXPIRY_SECS};
use super::events::handle_node_events;
use super::session::{now_secs, SessionState, SessionStateInner};
use super::state::ServiceState;
use super::storage::{delete_session, load_session, save_session};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};

/// Sentinel principal for sessions owned by the node owner (non-key auth, e.g.
/// embedded username/password). All node-owner requests share one principal —
/// they are the same human — so they may freely reuse each other's sessions.
///
/// Both sentinels contain characters (`-`, `<`, `>`) outside the hex alphabet
/// that `PublicKey::to_string()` emits, so they can never collide with a real
/// key-derived principal. (They were outside base58 before, and are outside the
/// smaller hex alphabet too — the guarantee only got easier to keep.)
const NODE_OWNER_PRINCIPAL: &str = "node-owner";

/// Sentinel for a request that reached an auth-guarded service without any
/// authenticated principal. Never equal to a real owner, so ownership checks
/// fail closed instead of granting the single-tenant allowance.
const UNAUTHENTICATED_PRINCIPAL: &str = "<unauthenticated>";

/// Resolve the principal that owns (or is requesting) a session from the auth
/// guard's injected extensions.
///
/// - A verified Ed25519 key → that key's string form.
/// - Non-key auth (`AuthenticatedNodeOwner`) → the shared [`NODE_OWNER_PRINCIPAL`].
/// - Neither, auth **enabled** → [`UNAUTHENTICATED_PRINCIPAL`]: the guard is
///   running but injected no principal (bypassed / mounted elsewhere). Fail
///   closed rather than silently granting access.
/// - Neither, auth **disabled** → `None`: single-tenant local node, ownership
///   not enforced.
fn caller_principal(
    auth_key: Option<&AuthenticatedKey>,
    auth_node_owner: Option<&AuthenticatedNodeOwner>,
    auth_enabled: bool,
) -> Option<String> {
    if let Some(AuthenticatedKey(pk)) = auth_key {
        Some(pk.to_string())
    } else if auth_node_owner.is_some() {
        Some(NODE_OWNER_PRINCIPAL.to_owned())
    } else if auth_enabled {
        Some(UNAUTHENTICATED_PRINCIPAL.to_owned())
    } else {
        None
    }
}

/// Whether `caller` may access a session owned by `session_owner`.
///
/// - `(Some, Some)` → allowed only when the principals match. A mismatch between
///   two known principals — including an owned session reached by the
///   [`UNAUTHENTICATED_PRINCIPAL`] sentinel — is a cross-principal access attempt
///   (IDOR) and is refused.
/// - `(Some, None)` → allowed, but logged. This is only reachable with auth
///   **disabled** at access time ([`caller_principal`] never returns `None` once
///   auth is enabled). It means an owned session — created while auth was on —
///   is being accessed after auth was turned off. Ownership is not enforced on a
///   single-tenant node, but the access is surfaced via `warn!` so an
///   auth-disabling configuration change that exposes previously-owned sessions
///   is visible in logs/audit rather than silent.
/// - `(None, _)` → allowed. An unowned session (legacy, or created with auth
///   disabled) has no principal to protect, so any caller may use it — including
///   the [`UNAUTHENTICATED_PRINCIPAL`] sentinel. The sentinel is fail-closed only
///   for *owned* sessions, which is the IDOR case being defended.
fn owner_allows_access(session_owner: &Option<String>, caller: &Option<String>) -> bool {
    match (session_owner, caller) {
        (Some(owner), Some(caller)) => owner == caller,
        (Some(owner), None) => {
            warn!(
                %owner,
                "SSE session ownership not enforced: an owned session was accessed with no \
                 caller principal (auth is disabled at access time)",
            );
            true
        }
        (None, _) => true,
    }
}

/// Seed `session`'s currently-bound connection with the live presence of every
/// context in `contexts`.
///
/// The contexts must already have passed the observation gate AND been
/// recorded on the session — this runs *after* the subscription is live, so
/// nothing that lands mid-flight is lost (see [`crate::ephemeral_replay`]).
///
/// Silent on failure: a session with no live connection (the client
/// subscribed, then dropped the stream) simply has nothing to seed, and a
/// dropped frame costs the client one heartbeat of staleness.
async fn replay_presence(
    state: &ServiceState,
    session: &SessionState,
    contexts: &[calimero_primitives::context::ContextId],
) {
    // Concurrent, not sequential: each context's snapshot read carries its own
    // timeout, and awaiting them in turn would stack those timeouts ahead of
    // the subscribe acknowledgment on a multi-context subscribe.
    for (context_id, events) in
        crate::ephemeral_replay::presence_replay_many(&state.node_client, contexts).await
    {
        for event in events {
            let body = match serde_json::to_value(&event) {
                Ok(value) => ResponseBody::Result(value),
                Err(err) => {
                    error!(%err, %context_id, "Failed to serialize presence replay event");
                    continue;
                }
            };
            if !session.try_push(SseResponse { body }) {
                debug!(%context_id, "Presence replay dropped: no live SSE connection or a full channel");
            }
        }
    }
}

/// A 403 response for the subscription endpoint, matching its `(StatusCode,
/// Json<SseResponse>)` return shape.
fn forbidden() -> (StatusCode, Json<SseResponse>) {
    (
        StatusCode::FORBIDDEN,
        Json(SseResponse {
            body: ResponseBody::Error(ResponseBodyError::HandlerError(
                "Forbidden: not the session owner".into(),
            )),
        }),
    )
}

/// Handle subscription/unsubscription requests
pub async fn handle_subscription(
    Extension(state): Extension<Arc<ServiceState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    auth_node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    Json(request): Json<Request<serde_json::Value>>,
) -> impl IntoResponse {
    let caller = caller_principal(
        auth_key.as_deref(),
        auth_node_owner.as_deref(),
        state.auth_enabled,
    );
    let session_id = match request.id.parse::<ConnectionId>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SseResponse {
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

            // Clone the session handle out of the map and release the map lock
            // immediately — the map read-lock must not span the store write below.
            let session = state.sessions.read().await.get(&session_id).cloned();

            if let Some(session) = session {
                // Serialize persistence for this session so concurrent
                // subscribe/unsubscribe commit to the store in mutation order.
                // The data lock (`inner`) is taken only to mutate + snapshot
                // (no I/O) and released before the blocking `save_session`, so a
                // slow store write can't stall event delivery, which reads
                // `inner`. Lock order is persist-guard → `inner`.
                let _persist = session.persist_guard().await;

                // Enforce session ownership FIRST, before any per-context work or
                // logging. Otherwise a caller who guesses a session id they don't
                // own could still trigger membership-probe lookups and log lines
                // for arbitrary context ids. Ownership is immutable for a session,
                // so a short read lock here is sufficient.
                {
                    let inner = session.inner.read().await;
                    if !owner_allows_access(&inner.owner, &caller) {
                        warn!(%session_id, "SSE subscribe denied: caller is not the session owner");
                        return forbidden();
                    }
                }

                // Only subscribe to contexts this caller may observe; context events
                // carry state, so a non-member must not receive them. Unauthorized
                // ids are dropped and the response reflects only what was subscribed.
                let node_owner = auth_node_owner.is_some();
                let subscribed: Vec<_> = ctxs
                    .context_ids
                    .iter()
                    .copied()
                    .filter(|ctx| {
                        let caller = auth_key.as_ref().map(|Extension(AuthenticatedKey(pk))| pk);
                        let authorized = crate::ws::caller_may_observe_context(
                            &state.ctx_client,
                            state.auth_enabled,
                            node_owner,
                            caller,
                            ctx,
                        );
                        if !authorized {
                            warn!(%session_id, context_id=%ctx, "SSE subscribe denied: caller is not a member of the context");
                        }
                        authorized
                    })
                    .collect();

                // Authorize by effective (deny-list-aware) group membership, not
                // is_member: a kicked inherited member keeps a path but is denied.
                // Subscribe-time only, like may_observe_context. Admin authority
                // is resolved in the same pass, since admin-only payloads ride
                // the same subscription.
                let caller_key = auth_key.as_ref().map(|Extension(AuthenticatedKey(pk))| pk);
                let groups = crate::ws::authorize_group_subscriptions(
                    &state.ctx_client,
                    state.auth_enabled,
                    node_owner,
                    caller_key,
                    ctxs.group_ids.iter().copied(),
                );
                for group_id in &groups.denied {
                    warn!(%session_id, group_id=%group_id, "SSE subscribe denied: caller is not a member of the group");
                }

                let persisted = {
                    let mut guard = session.inner.write().await;
                    let inner = &mut *guard;
                    for ctx in &subscribed {
                        let _ = inner.subscriptions.insert(*ctx);
                    }
                    groups.apply(
                        &mut inner.group_subscriptions,
                        &mut inner.admin_group_subscriptions,
                    );
                    inner.touch();
                    inner.to_persisted()
                };
                let subscribed_groups = groups.subscribed;

                let mut store = state.store.clone();
                if let Err(err) = save_session(&mut store, session_id, &persisted) {
                    error!(%session_id, %err, "Failed to persist session subscriptions");
                }
                drop(_persist);

                // Seed this session's connection with each context's CURRENT
                // presence, now that the subscription is live and deltas are
                // flowing. Subscribe-then-seed (never the reverse) so a delta
                // landing in between is delivered rather than dropped; see
                // `crate::ephemeral_replay` for why that direction is the safe
                // one. Delivery goes to this session's own connection sink, so
                // no other subscriber sees this client's seed.
                replay_presence(&state, &session, &subscribed).await;

                (
                    StatusCode::OK,
                    Json(SseResponse {
                        body: ResponseBody::Result(serde_json::json!({
                            "status": "subscribed",
                            "contexts": subscribed,
                            // Hex-encoded to match the id representation the
                            // client subscribed with (and the group admin API).
                            "groups": subscribed_groups
                                .iter()
                                .map(|g| hex::encode(g.as_bytes()))
                                .collect::<Vec<_>>(),
                        })),
                    }),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(SseResponse {
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

            let session = state.sessions.read().await.get(&session_id).cloned();
            if let Some(session) = session {
                // See the subscribe path: serialize persistence per session and
                // keep the store write off the data lock.
                let _persist = session.persist_guard().await;

                let mut unsubscribed = Vec::new();
                let persisted = {
                    let mut inner = session.inner.write().await;
                    if !owner_allows_access(&inner.owner, &caller) {
                        warn!(%session_id, "SSE unsubscribe denied: caller is not the session owner");
                        return forbidden();
                    }

                    // Remove contexts that were actually subscribed. Idempotent:
                    // unsubscribing from a context that wasn't subscribed is fine.
                    for ctx in &ctxs.context_ids {
                        if inner.subscriptions.remove(ctx) {
                            unsubscribed.push(*ctx);
                        }
                    }
                    for gid in &ctxs.group_ids {
                        let _ = inner.group_subscriptions.remove(gid);
                        let _ = inner.admin_group_subscriptions.remove(gid);
                    }
                    inner.touch();
                    inner.to_persisted()
                };

                let mut store = state.store.clone();
                if let Err(err) = save_session(&mut store, session_id, &persisted) {
                    error!(%session_id, %err, "Failed to persist session after unsubscribe");
                }
                drop(_persist);

                // Idempotent operation - always return OK with info about what was unsubscribed
                // Response includes:
                // - "unsubscribed": contexts that were actually removed from subscriptions
                // - "requested": contexts that the client requested to unsubscribe from
                // Clients can compare these to detect contexts they weren't subscribed to
                (
                    StatusCode::OK,
                    Json(SseResponse {
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
                    Json(SseResponse {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Session not found. Please reconnect to SSE endpoint first.".into(),
                        )),
                    }),
                )
            }
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(SseResponse {
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
///
/// Clients observing gaps in event IDs should re-query application state as needed.
pub async fn sse_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    request: AxumRequest,
) -> impl IntoResponse {
    let headers = request.headers();

    // Check for Last-Event-ID header for reconnection
    // Format: "{session_id}-{event_number}"
    // We extract the session_id to restore subscriptions and counter position
    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split('-').next())
        .and_then(|id| id.parse::<ConnectionId>().ok());

    // Principal of the reconnecting client, used to refuse adopting a session
    // owned by a different principal (session-hijack via guessed Last-Event-ID).
    let caller = caller_principal(
        request.extensions().get::<AuthenticatedKey>(),
        request.extensions().get::<AuthenticatedNodeOwner>(),
        state.auth_enabled,
    );

    let (commands_sender, commands_receiver) =
        mpsc::channel::<Command>(COMMAND_CHANNEL_BUFFER_SIZE);

    let (session_id, session_state, is_reconnect) = if let Some(existing_session_id) = last_event_id
    {
        // Attempt to reconnect to existing session
        let sessions = state.sessions.read().await;
        if let Some(existing_session) = sessions.get(&existing_session_id).cloned() {
            // Check expiry + ownership while holding the session lock.
            let inner = existing_session.inner.read().await;
            if inner.is_expired() {
                drop(inner);
                drop(sessions);
                warn!(%existing_session_id, "Session expired, creating new session");
                create_new_session(&state, caller).await
            } else if !owner_allows_access(&inner.owner, &caller) {
                drop(inner);
                drop(sessions);
                warn!(%existing_session_id, "SSE reconnect denied: caller is not the session owner; issuing a fresh session");
                create_new_session(&state, caller).await
            } else {
                drop(inner);
                info!(%existing_session_id, "Client reconnecting to existing session (from cache)");
                (existing_session_id, existing_session, true)
            }
        } else {
            drop(sessions);
            // Try to load from persistent storage
            match load_session(&state.store, existing_session_id) {
                Ok(Some(persisted_data)) => {
                    // Check if session expired
                    if now_secs().saturating_sub(persisted_data.last_activity) > SESSION_EXPIRY_SECS
                    {
                        warn!(%existing_session_id, "Persisted session expired, creating new session");
                        // Clean up expired session
                        let mut store = state.store.clone();
                        drop(delete_session(&mut store, existing_session_id));
                        create_new_session(&state, caller).await
                    } else if !owner_allows_access(&persisted_data.owner, &caller) {
                        warn!(%existing_session_id, "SSE reconnect denied: caller is not the persisted session owner; issuing a fresh session");
                        create_new_session(&state, caller).await
                    } else {
                        info!(%existing_session_id, "Client reconnecting to persisted session");
                        // Restore session from storage
                        let session_state =
                            SessionState::new(SessionStateInner::from_persisted(persisted_data));
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
                    create_new_session(&state, caller).await
                }
                Err(err) => {
                    error!(%existing_session_id, %err, "Failed to load session from storage, creating new session");
                    create_new_session(&state, caller).await
                }
            }
        }
    } else {
        // New connection, create new session
        create_new_session(&state, caller).await
    };

    if is_reconnect {
        info!(%session_id, "Client reconnected, subscriptions restored");
    } else {
        debug!(%session_id, "New client session established");
    }

    // Convert commands to SSE events with event IDs
    let event_counter = Arc::clone(&session_state.inner);
    let command_stream = ReceiverStream::new(commands_receiver).then(move |command| {
        let event_counter = Arc::clone(&event_counter);
        async move {
            let event_id = event_counter
                .read()
                .await
                .event_counter
                .fetch_add(1, Ordering::SeqCst);
            let id_str = format!("{session_id}-{event_id}");

            match command {
                Command::Close(reason) => {
                    // Send close as standard "message" type with metadata
                    let close_data = serde_json::json!({
                        "type": "close",
                        "reason": reason
                    });
                    Ok::<Event, Infallible>(
                        Event::default()
                            .event(SseEvent::Message.as_str())
                            .id(id_str)
                            .data(close_data.to_string()),
                    )
                }
                Command::Send(response) => match to_json_string(&response) {
                    Ok(message) => Ok::<Event, Infallible>(
                        Event::default()
                            .event(SseEvent::Message.as_str())
                            .id(id_str)
                            .data(message),
                    ),
                    Err(err) => {
                        error!(%err, "Failed to serialize SseResponse");
                        let error_response = SseResponse {
                            body: ResponseBody::Error(ResponseBodyError::ServerError(
                                ServerResponseError::InternalError { err: None },
                            )),
                        };
                        // This is a static struct with no dynamic fields and
                        // the only non-trivial field is #[serde(skip)], so
                        // serialization cannot fail.
                        let data = to_json_string(&error_response)
                            .expect("static InternalError response must serialize");
                        Ok::<Event, Infallible>(
                            Event::default()
                                .event(SseEvent::Message.as_str())
                                .id(id_str)
                                .data(data),
                        )
                    }
                },
            }
        }
    });

    // Initial connection event with retry configuration
    // Note: Sent as first event in stream, but background handlers spawn concurrently
    // Uses standard "message" type so browsers' EventSource.onmessage catches it
    let connect_data = serde_json::json!({
        "type": "connect",
        "session_id": session_id.to_string(),
        "reconnect": is_reconnect
    });
    let initial_event = Event::default()
        .event(SseEvent::Message.as_str()) // Standard browser-compatible event type
        .id(format!("{session_id}-0"))
        .retry(retry_timeout())
        .data(connect_data.to_string());
    let initial_stream = stream::once(async { Ok::<Event, Infallible>(initial_event) });

    let stream = initial_stream.chain(command_stream);

    // Spawn event handler (after stream setup to ensure command channel is ready).
    // The handler is bound to this connection's command channel and exits on its
    // own when the connection closes, so there is no separate cleanup task to spawn.
    //
    // `commands_sender` is moved (not cloned) into the task on purpose: the only
    // remaining sender then lives inside the handler, while the receiver is owned
    // by the SSE response stream (`command_stream` above). When the client
    // disconnects, axum drops the response body and therefore the receiver, which
    // makes `command_sender.closed()` resolve and the handler exit. (axum streams
    // the SSE body lazily via `Sse::new`, so the receiver is not held alive by the
    // handler future itself.)
    // Bind the task to the session, aborting any task a previous connection
    // left running for the same session. Two live tasks would share this
    // session's `event_counter` and each bump it per broadcast event, corrupting
    // event IDs and double-delivering events.
    //
    // The sink is registered on the session (weakly — see
    // `SessionState::connection`) at the same time, so the subscribe POST,
    // which is a different request entirely, can seed THIS connection with the
    // context's current presence without broadcasting it to every other client.
    let connection_sink = commands_sender.downgrade();
    let event_task = tokio::spawn(handle_node_events(
        session_id,
        Arc::clone(&state),
        session_state.clone(),
        commands_sender,
    ));
    session_state.bind_connection(event_task.abort_handle(), connection_sink);

    // Build response with session ID in header for easy client access
    let sse_response = Sse::new(stream).keep_alive(KeepAlive::default());

    // Convert to Response and add custom headers
    let mut response: AxumResponse = sse_response.into_response();
    let headers = response.headers_mut();

    // Add session ID header for easy client access (no need to parse from stream)
    if let Ok(header_value) = session_id.to_string().try_into() {
        drop(headers.insert("X-SSE-Session-ID", header_value));
    }

    // Add reconnect status header
    let reconnect_value = if is_reconnect { "true" } else { "false" };
    match reconnect_value.try_into() {
        Ok(header_value) => {
            drop(headers.insert("X-SSE-Reconnect", header_value));
        }
        Err(err) => {
            error!(
                %session_id,
                %err,
                "Failed to create X-SSE-Reconnect header; continuing SSE connection without it"
            );
        }
    }

    response
}

/// Get session information by ID
///
/// Returns session details including subscriptions and event counter.
/// Useful for clients that missed the initial connect event or want to verify session state.
pub async fn get_session_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    auth_node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    Path(session_id): Path<ConnectionId>,
) -> impl IntoResponse {
    debug!(%session_id, "GET session info request");
    let caller = caller_principal(
        auth_key.as_deref(),
        auth_node_owner.as_deref(),
        state.auth_enabled,
    );

    // Check in-memory sessions first
    let sessions = state.sessions.read().await;
    if let Some(session) = sessions.get(&session_id) {
        let inner = session.inner.read().await;

        // Check if expired
        if inner.is_expired() {
            drop(inner);
            drop(sessions);
            return (
                StatusCode::GONE,
                Json(SseResponse {
                    body: ResponseBody::Error(ResponseBodyError::HandlerError(
                        "Session expired".into(),
                    )),
                }),
            );
        }

        // Refuse cross-principal access (IDOR): a session may only be read by
        // the principal that created it.
        if !owner_allows_access(&inner.owner, &caller) {
            drop(inner);
            drop(sessions);
            warn!(%session_id, "GET session denied: caller is not the session owner");
            return forbidden();
        }

        let subscriptions: Vec<_> = inner.subscriptions.iter().copied().collect();
        let event_counter = inner.event_counter.load(Ordering::SeqCst);
        drop(inner);
        drop(sessions);

        return (
            StatusCode::OK,
            Json(SseResponse {
                body: ResponseBody::Result(serde_json::json!({
                    "session_id": session_id,
                    "subscriptions": subscriptions,
                    "event_counter": event_counter,
                    "status": "active"
                })),
            }),
        );
    }
    drop(sessions);

    // Try to load from persistent storage
    match load_session(&state.store, session_id) {
        Ok(Some(persisted_data)) => {
            // Check if expired
            use super::session::now_secs;
            if now_secs().saturating_sub(persisted_data.last_activity) > SESSION_EXPIRY_SECS {
                (
                    StatusCode::GONE,
                    Json(SseResponse {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Session expired".into(),
                        )),
                    }),
                )
            } else if !owner_allows_access(&persisted_data.owner, &caller) {
                warn!(%session_id, "GET session denied: caller is not the persisted session owner");
                forbidden()
            } else {
                let subscriptions: Vec<_> = persisted_data.subscriptions.iter().copied().collect();
                (
                    StatusCode::OK,
                    Json(SseResponse {
                        body: ResponseBody::Result(serde_json::json!({
                            "session_id": session_id,
                            "subscriptions": subscriptions,
                            "event_counter": persisted_data.event_counter,
                            "status": "persisted"
                        })),
                    }),
                )
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(SseResponse {
                body: ResponseBody::Error(ResponseBodyError::HandlerError(
                    "Session not found".into(),
                )),
            }),
        ),
        Err(err) => {
            error!(%session_id, %err, "Failed to load session from storage");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SseResponse {
                    body: ResponseBody::Error(ResponseBodyError::ServerError(
                        ServerResponseError::InternalError { err: None },
                    )),
                }),
            )
        }
    }
}

/// Create a new session with persistent storage, owned by `owner`.
async fn create_new_session(
    state: &ServiceState,
    owner: Option<String>,
) -> (ConnectionId, SessionState, bool) {
    loop {
        let session_id = random();
        let mut sessions = state.sessions.write().await;
        match sessions.entry(session_id) {
            Entry::Occupied(_) => continue,
            Entry::Vacant(entry) => {
                let session_state = SessionState::new(SessionStateInner::with_owner(owner.clone()));
                let _ = entry.insert(session_state.clone());

                // Persist new session to store
                let persisted = session_state.inner.read().await.to_persisted();
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

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PublicKey;

    use super::*;

    fn pk(b: u8) -> PublicKey {
        PublicKey::from([b; 32])
    }

    #[test]
    fn caller_principal_prefers_key_then_node_owner_then_none() {
        // A verified key wins and maps to its string form.
        let key = AuthenticatedKey(pk(1));
        assert_eq!(
            caller_principal(Some(&key), None, true),
            Some(pk(1).to_string()),
        );
        // A key takes precedence even if the node-owner marker is also present.
        assert_eq!(
            caller_principal(Some(&key), Some(&AuthenticatedNodeOwner), true),
            Some(pk(1).to_string()),
        );
        // Non-key auth collapses to the shared node-owner principal.
        assert_eq!(
            caller_principal(None, Some(&AuthenticatedNodeOwner), true),
            Some(NODE_OWNER_PRINCIPAL.to_owned()),
        );
        // Auth enabled but no principal → fail-closed sentinel (never matches a
        // real owner).
        assert_eq!(
            caller_principal(None, None, true),
            Some(UNAUTHENTICATED_PRINCIPAL.to_owned()),
        );
        // Auth disabled: no principal to bind to (single-tenant allowance).
        assert_eq!(caller_principal(None, None, false), None);
    }

    #[test]
    fn auth_enabled_request_without_principal_is_denied_on_owned_session() {
        // The fail-closed sentinel must not be able to read an owned session.
        let owner = Some(pk(7).to_string());
        let unauth = caller_principal(None, None, true);
        assert_eq!(unauth, Some(UNAUTHENTICATED_PRINCIPAL.to_owned()));
        assert!(
            !owner_allows_access(&owner, &unauth),
            "an unauthenticated request under enabled auth must not access an owned session",
        );
        // It can still reach an unowned session (no owner to protect).
        assert!(owner_allows_access(&None, &unauth));
    }

    #[test]
    fn owner_binding_blocks_cross_principal_access() {
        let alice = Some(pk(1).to_string());
        let bob = Some(pk(2).to_string());

        // Same principal: allowed.
        assert!(owner_allows_access(&alice, &alice));
        // Different principals: denied (the IDOR case).
        assert!(!owner_allows_access(&alice, &bob));
        // node-owner principals are shared, so they match each other.
        let owner = Some(NODE_OWNER_PRINCIPAL.to_owned());
        assert!(owner_allows_access(&owner, &owner));
    }

    #[test]
    fn owner_binding_is_backward_compatible_when_unowned_or_unauthenticated() {
        let alice = Some(pk(1).to_string());

        // Unowned session (legacy/persisted-before-upgrade, or auth disabled at
        // creation): accessible by anyone.
        assert!(owner_allows_access(&None, &alice));
        // Auth disabled now (no caller principal): not blocked.
        assert!(owner_allows_access(&alice, &None));
        assert!(owner_allows_access(&None, &None));
    }

    // ----------------------------------------------------------------------
    // Presence replay on subscribe
    //
    // SSE splits subscribe (a POST) from the stream (a long-lived GET), so the
    // seed has to find its way from the POST handler to the one connection
    // bound to that session — and to no other. These tests pin that routing.
    // ----------------------------------------------------------------------

    use calimero_context_client::client::ContextClient;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;

    /// An SSE `ServiceState` whose node actor answers presence-snapshot reads
    /// with `snapshot`. The `TempDir` is returned so the blob store outlives
    /// the state.
    async fn sse_state_with(
        snapshot: Vec<crate::test_support::SnapshotEntry>,
    ) -> (Arc<ServiceState>, TempDir) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, blob_dir) = crate::test_support::test_node_client(
            &store,
            crate::test_support::stub_node_manager(snapshot),
            event_sender,
        )
        .await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());

        (
            Arc::new(ServiceState::new(node_client, ctx_client, store, false)),
            blob_dir,
        )
    }

    /// A session with a connection bound to a fresh command channel, as
    /// `sse_handler` does on a real connection.
    ///
    /// The receiver stands in for the SSE response stream and the returned
    /// sender for the one the connection's event task owns — the session itself
    /// holds only a weak reference, so BOTH must be kept alive by the caller for
    /// the connection to count as live. That is the production shape: the weak
    /// sink is exactly what makes a dead connection un-seedable.
    fn session_with_connection() -> (SessionState, mpsc::Sender<Command>, mpsc::Receiver<Command>) {
        let session = SessionState::new(SessionStateInner::default());
        let (tx, rx) = mpsc::channel::<Command>(16);
        let noop = tokio::spawn(async {});
        session.bind_connection(noop.abort_handle(), tx.downgrade());
        (session, tx, rx)
    }

    /// Pull the `Ephemeral` payloads out of whatever is queued on a channel.
    fn drain_ephemeral(rx: &mut mpsc::Receiver<Command>) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        while let Ok(Command::Send(response)) = rx.try_recv() {
            let ResponseBody::Result(value) = response.body else {
                continue;
            };
            if value.get("type") == Some(&serde_json::json!("Ephemeral")) {
                out.push(value);
            }
        }
        out
    }

    // The seed goes to the session that subscribed, and to nobody else. Routing
    // it through the node-wide broadcast instead would deliver one client's
    // seed to every other connected client.
    #[actix::test]
    async fn presence_replay_reaches_only_the_subscribing_session() {
        let author = PublicKey::from([0xA1; 32]);
        let ctx = ContextId::from([0x31; 32]);
        let (state, _blob_dir) = sse_state_with(vec![(author, vec![1, 2, 3], 1_500)]).await;

        let (subscriber, _subscriber_tx, mut subscriber_rx) = session_with_connection();
        let (_bystander, _bystander_tx, mut bystander_rx) = session_with_connection();

        replay_presence(&state, &subscriber, &[ctx]).await;

        let seeded = drain_ephemeral(&mut subscriber_rx);
        assert_eq!(seeded.len(), 1, "the subscribing session must be seeded");
        assert_eq!(
            seeded[0]["data"]["author"],
            serde_json::json!(author.to_string())
        );
        assert_eq!(seeded[0]["data"]["state"], serde_json::json!([1, 2, 3]));
        assert_eq!(seeded[0]["contextId"], serde_json::json!(ctx));

        assert!(
            drain_ephemeral(&mut bystander_rx).is_empty(),
            "another session's seed must not reach an unrelated connection",
        );
    }

    // The replayed entry carries its age; the live path (asserted in the WS
    // suite and in `calimero-primitives`) omits the field entirely.
    #[actix::test]
    async fn replayed_entry_carries_age() {
        let author = PublicKey::from([0xA2; 32]);
        let ctx = ContextId::from([0x32; 32]);
        let (state, _blob_dir) = sse_state_with(vec![(author, vec![9], 4_200)]).await;

        let (session, _tx, mut rx) = session_with_connection();
        replay_presence(&state, &session, &[ctx]).await;

        let seeded = drain_ephemeral(&mut rx);
        assert_eq!(seeded[0]["data"]["ageMs"], serde_json::json!(4_200));
    }

    // A session whose connection has gone away (the client dropped the stream)
    // has nothing to seed. The weak sink must report that rather than resurrect
    // a dead channel and queue frames nobody will ever read.
    #[actix::test]
    async fn a_session_with_no_live_connection_is_not_seeded() {
        let author = PublicKey::from([0xA3; 32]);
        let ctx = ContextId::from([0x33; 32]);
        let (state, _blob_dir) = sse_state_with(vec![(author, vec![1], 10)]).await;

        // Drop BOTH halves: the client's stream is gone and so is the event
        // task that owned the only strong sender.
        let (session, tx, rx) = session_with_connection();
        drop(rx);
        drop(tx);

        // No panic, no hang, and nothing queued: `try_push` fails closed.
        replay_presence(&state, &session, &[ctx]).await;
        assert!(
            !session.try_push(SseResponse {
                body: ResponseBody::Result(serde_json::json!({})),
            }),
            "a session with no live connection must refuse the push",
        );
    }

    // A RE-subscribe to a context the session is already subscribed to must
    // seed again. This is the whole reconnect story: `mero-js` re-POSTs
    // `subscribe` with every remembered context id after each `connect` frame,
    // and on a reconnect that adopts the surviving session (`Last-Event-ID`)
    // those ids are already in `inner.subscriptions`. If the handler seeded
    // only newly-added ids, a reconnecting client would get no seed at all and
    // `mero-react`'s replay-is-authoritative reconciliation would have nothing
    // to reconcile against, leaving ghost peers forever.
    //
    // The guarantee comes from `subscribed` being the authorized subset of the
    // REQUEST's ids, never a delta against the session's existing set.
    #[actix::test]
    async fn a_repeat_subscribe_seeds_again() {
        let author = PublicKey::from([0xA5; 32]);
        let ctx = ContextId::from([0x35; 32]);
        let (state, _blob_dir) = sse_state_with(vec![(author, vec![7], 30)]).await;

        // A session registered in the map, as `create_new_session` leaves it,
        // with a live connection bound.
        let (session, _tx, mut rx) = session_with_connection();
        let session_id: ConnectionId = 42;
        drop(
            state
                .sessions
                .write()
                .await
                .insert(session_id, session.clone()),
        );

        let subscribe = || {
            handle_subscription(
                Extension(Arc::clone(&state)),
                None,
                None,
                Json(
                    serde_json::from_value(serde_json::json!({
                        "id": session_id.to_string(),
                        "method": "subscribe",
                        "params": { "contextIds": [ctx] },
                    }))
                    .expect("subscribe request parses"),
                ),
            )
        };

        drop(subscribe().await);
        assert_eq!(
            drain_ephemeral(&mut rx).len(),
            1,
            "the first subscribe must seed",
        );

        // Same session, same context, already subscribed — the reconnect shape.
        drop(subscribe().await);
        assert_eq!(
            drain_ephemeral(&mut rx).len(),
            1,
            "a repeat subscribe must seed again, or a reconnecting client never gets its seed",
        );

        // The subscription set is still just the one context: seeding again is
        // not the same as double-recording the subscription.
        assert_eq!(session.inner.read().await.subscriptions.len(), 1);
    }

    // Rebinding a session to a new connection (the SSE reconnect path) must
    // redirect the seed to the NEW connection — a seed delivered to the stale
    // one would be invisible to the client that asked for it.
    #[actix::test]
    async fn rebinding_a_session_redirects_the_seed_to_the_new_connection() {
        let author = PublicKey::from([0xA4; 32]);
        let ctx = ContextId::from([0x34; 32]);
        let (state, _blob_dir) = sse_state_with(vec![(author, vec![5], 20)]).await;

        let (session, _first_tx, mut first_rx) = session_with_connection();

        let (second_tx, mut second_rx) = mpsc::channel::<Command>(16);
        let noop = tokio::spawn(async {});
        session.bind_connection(noop.abort_handle(), second_tx.downgrade());

        replay_presence(&state, &session, &[ctx]).await;

        assert!(
            drain_ephemeral(&mut first_rx).is_empty(),
            "the superseded connection must not be seeded",
        );
        assert_eq!(
            drain_ephemeral(&mut second_rx).len(),
            1,
            "the current connection must be seeded",
        );
    }
}
