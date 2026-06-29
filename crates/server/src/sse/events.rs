use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Response, ResponseBody, ResponseBodyError, ServerResponseError,
};
use core::pin::pin;
use futures_util::StreamExt;
use serde_json::to_value as to_json_value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error};

use super::session::SessionState;
use super::state::ServiceState;

/// Handle incoming node events and forward to subscribed clients
///
/// # Lifetime
///
/// This task is bound to a single SSE connection via `command_sender`. It runs
/// for as long as that connection is open and **exits as soon as the connection
/// closes** (the SSE stream's receiver is dropped) or the node's event stream
/// ends. Exiting promptly is important: the task holds a broadcast receiver
/// subscription obtained from [`NodeClient::receive_events`], so a task that
/// outlived its connection would leak that subscription — and the spawned task
/// itself — for the remaining lifetime of the process. On reconnection, a fresh
/// task is spawned and bound to the new connection.
///
/// # Event Delivery Behavior
///
/// This handler uses a **skip-on-disconnect** model:
/// - Events are only delivered while the connection is active
/// - Events that occur during disconnection are **not buffered** and will be skipped
/// - When a client reconnects, they resume from the current event counter
/// - Clients should handle gaps in event IDs and re-query application state if needed
///
/// This design prioritizes simplicity and resource efficiency over guaranteed delivery.
/// For critical state updates, clients should implement their own state reconciliation
/// after reconnection.
pub async fn handle_node_events(
    session_id: ConnectionId,
    state: Arc<ServiceState>,
    session_state: SessionState,
    command_sender: mpsc::Sender<Command>,
) {
    let events = state.node_client.receive_events();

    let mut events = pin!(events);

    loop {
        let event = tokio::select! {
            // Poll the close branch first so a closed connection is detected
            // promptly even when the event stream is producing faster than the
            // channel drains; otherwise random branch selection could keep
            // starving the close branch and delay task exit.
            biased;
            // Stop as soon as the connection goes away so we don't leak the
            // broadcast receiver subscription (and this task) for the process
            // lifetime. The session itself persists for reconnection; a new
            // task is spawned when the client reconnects.
            () = command_sender.closed() => {
                debug!(%session_id, "SSE connection closed, stopping event handler");
                break;
            }
            maybe_event = events.next() => match maybe_event {
                Some(event) => event,
                None => {
                    debug!(%session_id, "Node event stream ended, stopping event handler");
                    break;
                }
            },
        };

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

        let body = match to_json_value(event) {
            Ok(v) => ResponseBody::Result(v),
            Err(err) => {
                error!(%session_id, %err, "Failed to serialize node event");
                ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: None },
                ))
            }
        };

        let response = Response { body };

        if let Err(err) = command_sender.send(Command::Send(response)).await {
            // The receiver is gone, so the connection has closed. Stop here
            // rather than spinning; the session persists for reconnection.
            debug!(
                %session_id,
                %err,
                "Failed to send event (connection closed), stopping event handler",
            );
            break;
        };
    }
}
