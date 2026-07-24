//! Bridges the governance `OpEvent` bus to the client-facing `NodeEvent` bus for
//! group-membership changes. Structural twin of [`crate::auto_follow`]; observational only.

use std::sync::Mutex;

use calimero_governance_store::op_events::{self, OpEvent};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::events::{
    GroupMembershipEvent, MembershipChange, MembershipChangePayload, NodeEvent,
};
use calimero_primitives::hash::Hash;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

/// Process-wide handle to the running bridge, so a re-`spawn` (e.g. after an
/// actor restart) does not double-subscribe and double-deliver every event.
static HANDLE: Mutex<Option<AbortHandle>> = Mutex::new(None);

/// Spawn the membership-event bridge. Idempotent: a no-op while a prior task
/// is still running.
pub fn spawn(node_client: NodeClient) {
    let mut slot = HANDLE.lock().expect("membership-events HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.is_finished()) {
        debug!("membership-events bridge already running; skipping re-spawn");
        return;
    }
    // Subscribe synchronously, before spawning, so no event emitted right after can be missed.
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        run(rx, node_client).await;
    })
    .abort_handle();
    *slot = Some(abort);
}

/// Abort the running bridge. For tests and graceful shutdown; safe to call when
/// nothing is running. After this, [`spawn`] may be called again.
pub fn shutdown() {
    if let Some(abort) = HANDLE
        .lock()
        .expect("membership-events HANDLE poisoned")
        .take()
    {
        abort.abort();
    }
}

async fn run(mut rx: broadcast::Receiver<OpEvent>, node_client: NodeClient) {
    info!("membership-events bridge started");
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "membership-events bridge lagged; some events dropped. The DAG is \
                     authoritative - a client can re-query the member list to reconcile."
                );
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                warn!("membership-events op-event channel closed; bridge exiting");
                break;
            }
        };
        if let Some((group_id, payload)) = to_membership_change(&event) {
            let node_event = NodeEvent::GroupMembership(GroupMembershipEvent {
                group_id: Hash::from(group_id),
                payload,
            });
            if let Err(err) = node_client.send_event(node_event) {
                debug!(?err, "membership-events: send_event failed (no receivers?)");
            }
        }
    }
}

/// Maps membership-carrying `OpEvent`s to a client-facing payload; `None` for
/// every other variant. `MemberLeft` is folded into `MemberRemoved` upstream.
fn to_membership_change(event: &OpEvent) -> Option<([u8; 32], MembershipChangePayload)> {
    match event {
        OpEvent::MemberJoined {
            group_id,
            member,
            role,
        } => Some((
            *group_id,
            MembershipChangePayload::MemberJoined(MembershipChange {
                member: *member,
                role: role.clone(),
            }),
        )),
        OpEvent::MemberAdded {
            group_id,
            member,
            role,
        } => Some((
            *group_id,
            MembershipChangePayload::MemberAdded(MembershipChange {
                member: *member,
                role: Some(role.clone()),
            }),
        )),
        OpEvent::MemberRemoved { group_id, member } => Some((
            *group_id,
            MembershipChangePayload::MemberRemoved(MembershipChange {
                member: *member,
                role: None,
            }),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;

    use super::*;

    #[test]
    fn maps_joined_added_removed_and_ignores_others() {
        let gid = [0x42; 32];
        let member = PublicKey::from([0x07; 32]);

        let (g, p) = to_membership_change(&OpEvent::MemberJoined {
            group_id: gid,
            member,
            role: None,
        })
        .expect("join maps");
        assert_eq!(g, gid);
        assert!(matches!(p, MembershipChangePayload::MemberJoined(_)));

        let (_, p) = to_membership_change(&OpEvent::MemberAdded {
            group_id: gid,
            member,
            role: GroupMemberRole::Admin,
        })
        .expect("add maps");
        match p {
            MembershipChangePayload::MemberAdded(c) => {
                assert_eq!(c.role, Some(GroupMemberRole::Admin));
            }
            other => panic!("expected MemberAdded, got {other:?}"),
        }

        let (_, p) = to_membership_change(&OpEvent::MemberRemoved {
            group_id: gid,
            member,
        })
        .expect("remove maps");
        assert!(matches!(p, MembershipChangePayload::MemberRemoved(_)));

        // A non-membership op is not bridged.
        assert!(to_membership_change(&OpEvent::GroupKeyDelivered {
            group_id: gid,
            recipient: member,
        })
        .is_none());
    }
}
