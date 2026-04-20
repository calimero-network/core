//! Process-wide broadcast of governance-DAG op-apply events.
//!
//! Downstream handlers subscribe to observe governance ops as they are
//! applied to local state. Every emitted event corresponds to a
//! successfully-applied op; this is the hook used by higher-level
//! features (notably auto-follow for group membership, see
//! `docs/adr/0001-auto-follow-group-membership.md`) to react to state
//! changes without reaching into the apply path.
//!
//! The channel is best-effort: slow subscribers that fall behind receive
//! `RecvError::Lagged` and are expected to reconcile from the DAG, which
//! is authoritative. No guaranteed delivery; no durable queue.
//!
//! The existing `crate::registration_notify` stays as a narrower signal
//! specialized for `ContextRegistered`; this module is its general-
//! purpose peer covering all op variants that downstream handlers care
//! about.

use std::sync::OnceLock;

use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use tokio::sync::broadcast;

/// Broadcast capacity. Bounds a burst of ops applied in quick succession
/// (e.g. a batch sync pulling 100 ops from a peer) without forcing
/// subscribers to lag in the common case. Subscribers that do lag get
/// `Lagged(n)` and are expected to re-read state from the DAG.
const CHANNEL_CAPACITY: usize = 1024;

/// A governance op was successfully applied to local state.
///
/// Only op variants that existing or planned downstream handlers react
/// to are represented. New variants are added as needed; adding one is
/// a non-breaking change since downstream matches on known variants
/// and ignores the rest.
#[derive(Clone, Debug)]
pub enum OpEvent {
    /// `RootOp::GroupNested` — a subgroup was nested under a parent.
    SubgroupNested {
        namespace_id: [u8; 32],
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// `RootOp::GroupUnnested` — a subgroup was detached from a parent.
    SubgroupUnnested {
        namespace_id: [u8; 32],
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// `GroupOp::ContextRegistered` — a new context was registered in a group.
    ContextRegistered {
        group_id: [u8; 32],
        context_id: ContextId,
    },
    /// `GroupOp::MemberAdded` — a member was added to a group by an admin.
    MemberAdded {
        group_id: [u8; 32],
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// `GroupOp::MemberJoinedViaTeeAttestation` — a TEE node was admitted.
    TeeMemberAdmitted {
        group_id: [u8; 32],
        member: PublicKey,
    },
    /// `GroupOp::MemberRemoved` — a member was removed from a group.
    MemberRemoved {
        group_id: [u8; 32],
        member: PublicKey,
    },
}

static NOTIFIER: OnceLock<broadcast::Sender<OpEvent>> = OnceLock::new();

fn sender() -> &'static broadcast::Sender<OpEvent> {
    NOTIFIER.get_or_init(|| broadcast::channel(CHANNEL_CAPACITY).0)
}

/// Emit an op-apply event. Best-effort: silently drops if there are no
/// subscribers or the channel is closed.
pub fn notify(event: OpEvent) {
    let _ = sender().send(event);
}

/// Subscribe to future op-apply events. Subscribe before triggering
/// work that might apply ops, otherwise events fired in the gap between
/// trigger and subscribe are missed (re-read state from the DAG to
/// recover).
pub fn subscribe() -> broadcast::Receiver<OpEvent> {
    sender().subscribe()
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::PublicKey;

    use super::*;

    #[tokio::test]
    async fn notify_delivers_to_subscriber() {
        let mut rx = subscribe();
        let context_id = ContextId::from([1u8; 32]);
        notify(OpEvent::ContextRegistered {
            group_id: [2u8; 32],
            context_id,
        });
        let event = rx.recv().await.expect("event delivered");
        match event {
            OpEvent::ContextRegistered { group_id, context_id: got } => {
                assert_eq!(group_id, [2u8; 32]);
                assert_eq!(got, context_id);
            }
            other => panic!("expected ContextRegistered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn notify_with_no_subscribers_is_silent() {
        // Must not panic or error with zero subscribers. Best-effort.
        notify(OpEvent::MemberRemoved {
            group_id: [3u8; 32],
            member: PublicKey::from([4u8; 32]),
        });
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let mut rx1 = subscribe();
        let mut rx2 = subscribe();
        notify(OpEvent::MemberAdded {
            group_id: [5u8; 32],
            member: PublicKey::from([6u8; 32]),
            role: GroupMemberRole::Member,
        });
        for rx in [&mut rx1, &mut rx2] {
            let event = rx.recv().await.expect("event delivered");
            matches!(event, OpEvent::MemberAdded { .. });
        }
    }
}
