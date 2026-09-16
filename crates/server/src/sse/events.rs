use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Response, ResponseBody, ResponseBodyError, ServerResponseError,
};
use core::pin::pin;
use futures_util::StreamExt;
use serde_json::to_value as to_json_value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

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
///
/// # Lifecycle (no task leak)
///
/// This task is scoped to a single connection and writes directly to that
/// connection's command channel (`commands_sender`). It exits **only** when the
/// connection actually goes away — either `commands_sender.closed()` fires (the
/// SSE stream's receiver was dropped) or a send fails for the same reason — or
/// when the node event stream ends. It does not consult `active_connections`,
/// so a transient miss there during a reconnect handoff cannot kill it.
/// Previously it slept-and-looped forever when no connection was active, which
/// leaked a task per disconnect.
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

        let (subscriptions, group_subscriptions, admin_group_subscriptions) = {
            let inner = session_state.inner.read().await;
            (
                inner.subscriptions.clone(),
                inner.group_subscriptions.clone(),
                inner.admin_group_subscriptions.clone(),
            )
        };

        debug!(
            %session_id,
            "Received node event: {:?}, subscriptions state: {:?}",
            event,
            subscriptions
        );

        // Captured before the match below consumes `event`. A removal is the
        // one event that can invalidate a subscription already granted, and it
        // drives the prune whether or not this session is subscribed to the
        // group it names: a removal from a parent group can revoke an inherited
        // member of a descendant, which this session may well be watching.
        let membership_revoked = matches!(
            &event,
            NodeEvent::GroupMembership(membership_event)
                if matches!(
                    membership_event.payload,
                    calimero_primitives::events::MembershipChangePayload::MemberRemoved(_)
                )
        );

        let event = match event {
            NodeEvent::Context(event) if subscriptions.contains(&event.context_id) => {
                NodeEvent::Context(event)
            }
            NodeEvent::Context(_) => continue,
            NodeEvent::GroupMembership(event) if group_subscriptions.contains(&event.group_id) => {
                NodeEvent::GroupMembership(event)
            }
            // Not delivered — this session does not watch the group the
            // removal names — but still pruned, because the removal may
            // revoke a subgroup this session DOES watch by inheritance. The
            // other two `continue` arms need no such call: only a
            // `GroupMembership` event can set `membership_revoked`.
            NodeEvent::GroupMembership(_) => {
                prune_revoked_subscriptions(session_id, &state, &session_state, membership_revoked)
                    .await;
                continue;
            }
            NodeEvent::GroupMigration(event)
                if crate::ws::may_deliver_group_event(
                    event.payload.requires_group_admin(),
                    &event.group_id,
                    &group_subscriptions,
                    &admin_group_subscriptions,
                ) =>
            {
                NodeEvent::GroupMigration(event)
            }
            NodeEvent::GroupMigration(_) => continue,
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

        // AFTER delivery, deliberately: the removed member is told they were
        // removed on the same stream the removal takes away from them. Pruning
        // first would drop the one event that explains the silence.
        prune_revoked_subscriptions(session_id, &state, &session_state, membership_revoked).await;
    }
}

/// Drop this session's subscriptions whose caller no longer passes the
/// subscribe-time gate.
///
/// Runs only on a membership REMOVAL (`revoked`), not on every event: that is
/// what makes it affordable. The cost is one re-authorization pass per removal
/// — a rare, governance-paced event — rather than a membership lookup per event
/// per subscriber, on a path that carries video frames and document updates.
///
/// Scoped to this session because the task is: SSE spawns one event task per
/// connection, so every connected session prunes itself and no session is
/// pruned twice. A session with NO live connection has no task and is not
/// pruned here — it does not need to be, because it is delivering nothing;
/// what protects it is that the reduced set is PERSISTED below, so the
/// subscriptions a reconnect restores are the ones that survived the last
/// prune, never the revoked ones.
async fn prune_revoked_subscriptions(
    session_id: ConnectionId,
    state: &ServiceState,
    session_state: &SessionState,
    revoked: bool,
) {
    if !revoked {
        return;
    }

    // Snapshot under a read lock; the membership lookups below touch the store
    // and must not run while holding it.
    let (caller, node_owner, subscriptions, group_subscriptions) = {
        let inner = session_state.inner.read().await;
        (
            inner.caller,
            inner.node_owner,
            inner.subscriptions.clone(),
            inner.group_subscriptions.clone(),
        )
    };
    if subscriptions.is_empty() && group_subscriptions.is_empty() {
        return;
    }

    let revocation = crate::ws::revoke_lost_subscriptions(
        &state.ctx_client,
        state.auth_enabled,
        node_owner,
        caller.as_ref(),
        &subscriptions,
        &group_subscriptions,
    );
    if revocation.is_empty() {
        return;
    }
    let (contexts, denied_groups, demoted_groups) = revocation.lost();
    warn!(
        %session_id,
        contexts = contexts.len(),
        groups = denied_groups.len(),
        demoted = demoted_groups.len(),
        "revoking SSE subscriptions: the caller no longer passes the observation gate",
    );

    // Same lock order the subscribe path uses: persist-guard, then `inner`,
    // and the store write happens outside `inner` so it cannot stall delivery.
    let _persist = session_state.persist_guard().await;
    let persisted = {
        let mut guard = session_state.inner.write().await;
        let inner = &mut *guard;
        revocation.apply(
            &mut inner.subscriptions,
            &mut inner.group_subscriptions,
            &mut inner.admin_group_subscriptions,
        );
        inner.to_persisted()
    };
    let mut store = state.store.clone();
    if let Err(err) = super::storage::save_session(&mut store, session_id, &persisted) {
        // The in-memory set is already reduced, so this connection stops
        // delivering either way; what a failed write costs is that a RECONNECT
        // could restore the revoked ids from the stale record. Loud for that
        // reason.
        error!(
            %session_id, %err,
            "Failed to persist revoked SSE subscriptions; a reconnect may restore them",
        );
    }
}
