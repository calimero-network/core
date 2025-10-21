use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Response, ResponseBody, ResponseBodyError, ServerResponseError,
};
use core::pin::pin;
use core::time::Duration;
use futures_util::StreamExt;
use serde_json::to_value as to_json_value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::debug;

use super::session::SessionState;
use super::state::ServiceState;

/// Handle incoming node events and forward to subscribed clients
pub async fn handle_node_events(
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
        let _ = session_state
            .inner
            .read()
            .await
            .event_counter
            .fetch_add(1, Ordering::SeqCst);

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

/// Clean up connection when SSE stream closes
pub async fn handle_connection_cleanup(
    session_id: ConnectionId,
    state: Arc<ServiceState>,
    command_sender: mpsc::Sender<Command>,
) {
    command_sender.closed().await;

    // Remove active connection but keep session for reconnection
    drop(state.active_connections.write().await.remove(&session_id));

    debug!(%session_id, "Active SSE connection closed (session persists for reconnection)");
}
