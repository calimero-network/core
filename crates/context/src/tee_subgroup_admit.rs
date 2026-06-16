//! Transparent per-subgroup TEE admission (Phase 1, Restricted subgroups).
//!
//! Runs on a node that holds a subgroup's group key (its creator/member).
//! Reacts to two governance events and admits the namespace's entitled TEE
//! member(s) into Restricted subgroups, reusing the verified verdict from the
//! namespace-root op log. Open subgroups are skipped (a root-admitted TEE node
//! reads them via inherited membership + the namespace key). See proposal.md §12d.

use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::AdmitTeeNodeRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{
    tee_admission_record, CapabilitiesRepository, GroupKeyring, MembershipRepository,
};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, error, info, warn};

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

/// Admit `member` (an entitled TEE identity) into `subgroup`, reusing the
/// verdict recorded at its namespace-root admission. Idempotent and best-effort:
/// logs and continues on any error. Delivery of the per-subgroup key happens
/// inside `admit_tee_node` and succeeds because the caller holds the key.
async fn admit_member_into_subgroup(
    store: &Store,
    context_client: &ContextClient,
    namespace_gid: &ContextGroupId,
    subgroup_gid: &ContextGroupId,
    member: &PublicKey,
) {
    // Idempotency: skip if already a direct member of the subgroup.
    match MembershipRepository::new(store).has_direct_member(subgroup_gid, member) {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            error!(?e, "tee-subgroup-admit: has_direct_member check failed");
            return;
        }
    }

    let record = match tee_admission_record(store, namespace_gid, member) {
        Ok(Some(r)) => r,
        Ok(None) => {
            // Member is not a root TEE member (or its admission op isn't local
            // yet). Nothing to reuse; the key pull / a later event will recover.
            return;
        }
        Err(e) => {
            error!(?e, "tee-subgroup-admit: reading admission record failed");
            return;
        }
    };

    if record.role != GroupMemberRole::ReadOnlyTee {
        return;
    }

    let req = AdmitTeeNodeRequest {
        group_id: *subgroup_gid,
        member: *member,
        quote_hash: record.quote_hash,
        mrtd: record.mrtd,
        rtmr0: record.rtmr0,
        rtmr1: record.rtmr1,
        rtmr2: record.rtmr2,
        rtmr3: record.rtmr3,
        tcb_status: record.tcb_status,
        // Production TEE admissions are real; the op-log record carries no
        // is_mock flag. Mock-quote test paths admit via the allowlisted mock
        // MRTD regardless (accept_mock + allowed_mrtd), so false is correct here.
        is_mock: false,
    };

    if let Err(e) = context_client.admit_tee_node(req).await {
        error!(
            ?e,
            "tee-subgroup-admit: admit_tee_node into subgroup failed (key pull is the fallback)"
        );
    }
}

async fn handle_new_subgroup(
    store: &Store,
    context_client: &ContextClient,
    namespace_id: [u8; 32],
    child_group_id: [u8; 32],
) {
    let namespace_gid = ContextGroupId::from(namespace_id);
    let child_gid = ContextGroupId::from(child_group_id);

    // Only act for Restricted subgroups — Open subgroups are already readable
    // by a root-admitted TEE node (inherited membership + namespace key).
    match CapabilitiesRepository::new(store).is_open_chain_to_namespace(&child_gid, &namespace_gid)
    {
        Ok(true) => return, // Open → skip
        Ok(false) => {}     // Restricted → proceed
        Err(e) => {
            error!(?e, "tee-subgroup-admit: open-chain check failed");
            return;
        }
    }

    // Only the key-holder (the creator) can deliver the per-subgroup key.
    match GroupKeyring::new(store, child_gid).load_current_key() {
        Ok(Some(_)) => {}   // we hold the key → we can admit + deliver
        Ok(None) => return, // not the key-holder → leave it to the creator / pull
        Err(e) => {
            error!(?e, "tee-subgroup-admit: load_current_key failed");
            return;
        }
    }

    // Admit every existing root-level ReadOnlyTee member into the new subgroup.
    let members = match MembershipRepository::new(store).list(&namespace_gid, 0, usize::MAX) {
        Ok(m) => m,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: listing root members failed");
            return;
        }
    };
    for (member, role) in members {
        if role == GroupMemberRole::ReadOnlyTee {
            admit_member_into_subgroup(store, context_client, &namespace_gid, &child_gid, &member)
                .await;
        }
    }
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
