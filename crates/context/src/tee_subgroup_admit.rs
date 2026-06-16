//! Transparent per-subgroup TEE admission (Phase 1, Restricted subgroups).
//!
//! Runs on a node that holds a subgroup's group key (its creator/member).
//! Reacts to two governance events and admits the namespace's entitled TEE
//! member(s) into Restricted subgroups, reusing the verified verdict from the
//! namespace-root op log. Open subgroups are skipped (a root-admitted TEE node
//! reads them via inherited membership + the namespace key). See proposal.md §12d.

use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AdmitTrigger {
    /// A new subgroup was created; admit the namespace's existing root TEE members.
    NewSubgroup {
        namespace_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// A TEE member was admitted somewhere; if it was the namespace root, fan it
    /// out into the Restricted subgroups this node holds keys for.
    NewTeeMember {
        group_id: [u8; 32],
        member: PublicKey,
    },
}

/// Pure event→intent mapping. The store-touching filtering (Restricted-ness,
/// key ownership, root-vs-subgroup, idempotency) happens in the handlers.
pub(crate) fn admit_trigger(event: &OpEvent) -> Option<AdmitTrigger> {
    match event {
        OpEvent::SubgroupCreated {
            namespace_id,
            child_group_id,
            ..
        } => Some(AdmitTrigger::NewSubgroup {
            namespace_id: *namespace_id,
            child_group_id: *child_group_id,
        }),
        OpEvent::TeeMemberAdmitted { group_id, member } => Some(AdmitTrigger::NewTeeMember {
            group_id: *group_id,
            member: *member,
        }),
        _ => None,
    }
}

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Spawn the tee-subgroup-admit handler. Returns immediately; the handler runs
/// as a detached tokio task for the process lifetime.
///
/// Idempotent: subsequent calls (e.g. after an Actix actor restart) are no-ops
/// unless [`shutdown`] is called first.
pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("tee-subgroup-admit HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("tee-subgroup-admit handler already running; skipping re-spawn");
        return;
    }
    let abort = tokio::spawn(async move {
        run(store, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

/// Abort the running handler task. Intended for tests and graceful-shutdown
/// hooks. Safe to call even if no handler is running. After calling this,
/// [`spawn`] may be called again.
pub fn shutdown() {
    if let Some(state) = HANDLE
        .lock()
        .expect("tee-subgroup-admit HANDLE poisoned")
        .take()
    {
        state.abort.abort();
    }
}

async fn run(store: Store, context_client: ContextClient) {
    let mut rx = op_events::subscribe();
    info!("tee-subgroup-admit handler started");
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "tee-subgroup-admit subscriber lagged; dropped events are recovered \
                     by the next SubgroupCreated/TeeMemberAdmitted or the joiner-side key pull"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("tee-subgroup-admit op-event channel closed; handler exiting");
                break;
            }
        };

        match admit_trigger(&event) {
            Some(AdmitTrigger::NewSubgroup {
                namespace_id,
                child_group_id,
            }) => handle_new_subgroup(&store, &context_client, namespace_id, child_group_id).await,
            Some(AdmitTrigger::NewTeeMember { group_id, member }) => {
                handle_new_tee_member(&store, &context_client, group_id, member).await
            }
            None => {}
        }
    }
}

// Stubs — implemented in Tasks 3 and 4.
async fn handle_new_subgroup(
    _store: &Store,
    _context_client: &ContextClient,
    _namespace_id: [u8; 32],
    _child_group_id: [u8; 32],
) {
}

async fn handle_new_tee_member(
    _store: &Store,
    _context_client: &ContextClient,
    _group_id: [u8; 32],
    _member: PublicKey,
) {
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use calimero_governance_store::op_events::OpEvent;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    #[test]
    fn maps_only_subgroup_created_and_tee_admitted() {
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();

        assert_eq!(
            admit_trigger(&OpEvent::SubgroupCreated {
                namespace_id: [1u8; 32],
                parent_group_id: [2u8; 32],
                child_group_id: [3u8; 32],
            }),
            Some(AdmitTrigger::NewSubgroup {
                namespace_id: [1u8; 32],
                child_group_id: [3u8; 32],
            })
        );

        assert_eq!(
            admit_trigger(&OpEvent::TeeMemberAdmitted {
                group_id: [4u8; 32],
                member,
            }),
            Some(AdmitTrigger::NewTeeMember {
                group_id: [4u8; 32],
                member,
            })
        );

        // Unrelated events are ignored.
        assert_eq!(
            admit_trigger(&OpEvent::MemberAdded {
                group_id: [5u8; 32],
                member,
                role: GroupMemberRole::Member,
            }),
            None
        );
        assert_eq!(
            admit_trigger(&OpEvent::TeeMemberRemoved {
                group_id: [6u8; 32],
                member,
            }),
            None
        );
    }
}
