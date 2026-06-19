//! Transparent per-subgroup TEE admission (Phase 1, Restricted subgroups).
//!
//! Runs on a node that holds a subgroup's group key (its creator/member).
//! Reacts to two governance events and admits the namespace's entitled TEE
//! member(s) into Restricted subgroups, reusing the verified verdict from the
//! namespace-root op log. Open subgroups are skipped (a root-admitted TEE node
//! reads them via inherited membership + the namespace key). See proposal.md §12d.
//!
//! Race note: the apply path emits `OpEvent::TeeMemberAdmitted` *before* it
//! persists the op-log entry, with no enclosing transaction (see the
//! "atomic batch deferred" note in `governance-store/src/local_state.rs`). A
//! subscriber reacting to that event can therefore observe `tee_admission_record`
//! returning `None` for a member it just learned about. `handle_new_tee_member`
//! works around this with a bounded wake-then-reread retry. So that this retry
//! (up to ~1s) never stalls the receive loop — which would let the bounded
//! `op_events` broadcast overflow and silently drop an unrelated
//! `SubgroupCreated`/`TeeMemberAdmitted` — `run` dispatches each event onto its
//! own task. The durable fix — emitting events only after the op-log entry is
//! persisted (ideally in one atomic batch) in the apply path — is a deferred
//! follow-up.

use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::AdmitTeeNodeRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{
    tee_admission_record, tee_admission_records, CapabilitiesRepository, GroupKeyring,
    MembershipRepository, NamespaceRepository, TeeAdmissionRecord,
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
/// Subscribes to `op_events` **synchronously, before** spawning the task, so
/// that once this returns no subsequently-emitted event can be missed. (If the
/// task subscribed lazily on its first poll, an event fired between `spawn`
/// returning and that poll would be lost — the race the e2e tests previously
/// papered over with a sleep.)
///
/// Idempotent: subsequent calls (e.g. after an Actix actor restart) are no-ops
/// while a handler is still running; [`shutdown`] first to rebind to a new
/// store/client. Re-spawning is safe regardless because admission is idempotent
/// (`has_direct_member` guards duplicate rows) and best-effort.
pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("tee-subgroup-admit HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("tee-subgroup-admit handler already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        run(rx, store, context_client).await;
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

async fn run(
    mut rx: tokio::sync::broadcast::Receiver<OpEvent>,
    store: Store,
    context_client: ContextClient,
) {
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

        let Some(trigger) = admit_trigger(&event) else {
            continue;
        };

        // Dispatch each event on its own task so a handler's bounded
        // wake-then-reread retry (`handle_new_tee_member` can sleep up to ~1s
        // waiting out the emit-before-persist race) never blocks the receive
        // loop. A blocked loop would let the bounded `op_events` broadcast
        // overflow and drop an unrelated event. Handlers are idempotent and
        // best-effort, so a late task (e.g. still running past a `shutdown`) is
        // harmless. `Store` and `ContextClient` are cheap, shared-backing clones.
        let store = store.clone();
        let context_client = context_client.clone();
        tokio::spawn(async move {
            match trigger {
                AdmitTrigger::NewSubgroup {
                    namespace_id,
                    child_group_id,
                } => {
                    handle_new_subgroup(&store, &context_client, namespace_id, child_group_id).await
                }
                AdmitTrigger::NewTeeMember { group_id, member } => {
                    handle_new_tee_member(&store, &context_client, group_id, member).await
                }
            }
        });
    }
}

/// Admit `member` (an entitled TEE identity) into `subgroup_gid`, reusing the
/// already-fetched `record` from its namespace-root admission. Idempotent and
/// best-effort: logs and continues on any error. Delivery of the per-subgroup
/// key happens inside `admit_tee_node` and succeeds because the caller holds the
/// key.
///
/// The caller fetches the [`TeeAdmissionRecord`] once and may reuse it across
/// multiple subgroups, so this borrows it and clones the `String` fields when
/// building the request.
async fn admit_member_into_subgroup(
    context_client: &ContextClient,
    store: &Store,
    subgroup_gid: &ContextGroupId,
    member: &PublicKey,
    record: &TeeAdmissionRecord,
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

    if record.role != GroupMemberRole::ReadOnlyTee {
        return;
    }

    let req = AdmitTeeNodeRequest {
        group_id: *subgroup_gid,
        member: *member,
        quote_hash: record.quote_hash,
        mrtd: record.mrtd.clone(),
        rtmr0: record.rtmr0.clone(),
        rtmr1: record.rtmr1.clone(),
        rtmr2: record.rtmr2.clone(),
        rtmr3: record.rtmr3.clone(),
        tcb_status: record.tcb_status.clone(),
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

    // Note on the same emit-before-persist race: unlike `handle_new_tee_member`,
    // the two store reads below need no retry. `SubgroupCreated` only fires on
    // the creator (it ran `create_group`), which mints the subgroup key and
    // writes its rows before the op is applied — so `load_current_key` already
    // sees it. And subgroup visibility defaults to Restricted in the absence of
    // a row, so `is_open_chain_to_namespace` fails safe (treats a not-yet-written
    // subgroup as Restricted → we proceed) rather than wrongly skipping it.

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

    // Collect the root-level ReadOnlyTee members to admit into the new subgroup.
    let members = match MembershipRepository::new(store).list(&namespace_gid, 0, usize::MAX) {
        Ok(m) => m,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: listing root members failed");
            return;
        }
    };
    let tee_members: Vec<PublicKey> = members
        .into_iter()
        .filter(|(_, role)| *role == GroupMemberRole::ReadOnlyTee)
        .map(|(member, _)| member)
        .collect();
    if tee_members.is_empty() {
        return; // nothing to admit — skip the op-log scan entirely
    }

    // One scan of the root op log for all verdicts, rather than re-scanning it
    // per member. No retry needed here: the root admission long predates this
    // subgroup creation, so its op-log entries are already persisted and readable.
    let records = match tee_admission_records(store, &namespace_gid) {
        Ok(r) => r,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: reading admission records failed");
            return;
        }
    };
    for member in tee_members {
        let Some(record) = records.get(&member) else {
            continue; // no verdict to reuse (membership row without a join op)
        };
        admit_member_into_subgroup(context_client, store, &child_gid, &member, record).await;
    }
}

async fn handle_new_tee_member(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    member: PublicKey,
) {
    let group_gid = ContextGroupId::from(group_id);

    // Resolve the namespace root. React ONLY to root admissions — a subgroup
    // admission (which this very handler causes via admit_tee_node →
    // TeeMemberAdmitted) also fires TeeMemberAdmitted; ignoring non-root
    // admissions prevents an infinite fan-out loop.
    let namespace_gid = match NamespaceRepository::new(store).resolve(&group_gid) {
        Ok(ns) => ns,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: namespace resolve failed");
            return;
        }
    };
    if namespace_gid != group_gid {
        return; // subgroup admission echo → ignore
    }

    // The TeeMemberAdmitted event is emitted before the op-log entry is
    // persisted (apply path emits pre-persist; see join_group.rs for the same
    // wake-then-reread idiom). Retry briefly so the just-admitted member's
    // verdict becomes readable. The triggering event GUARANTEES the record will
    // exist imminently (it is the very admission that fired this event); the real
    // gap is microseconds, so 20×50ms = 1s is a generous upper bound.
    let mut record = None;
    for _ in 0..20 {
        match tee_admission_record(store, &namespace_gid, &member) {
            Ok(Some(r)) => {
                record = Some(r);
                break;
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            Err(e) => {
                error!(?e, "tee-subgroup-admit: reading admission record failed");
                return;
            }
        }
    }
    let Some(record) = record else {
        // Exhausting the retry budget is not expected (the verdict gap is
        // normally microseconds). If the store write is delayed past ~1s (heavy
        // load, disk pressure, compaction), the fan-in is dropped here and only
        // the joiner-side key pull recovers it. Log at error so operators can
        // detect the degraded path; the durable fix (emit events post-persist)
        // is #2770.
        error!(
            member = ?member,
            "tee-subgroup-admit: verdict not visible after retry budget; \
             fan-in deferred to the joiner-side key pull (see #2770)"
        );
        return;
    };

    // Enumerate Restricted subgroups of the namespace that this node holds keys
    // for, and admit the new member into each.
    let descendants = match NamespaceRepository::new(store).collect_descendants(&namespace_gid) {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: collect_descendants failed");
            return;
        }
    };

    let caps = CapabilitiesRepository::new(store);
    for sub in descendants {
        // Restricted only — Open subgroups are already readable by a
        // root-admitted TEE node (inherited membership + namespace key).
        match caps.is_open_chain_to_namespace(&sub, &namespace_gid) {
            Ok(true) => continue, // Open → skip
            Ok(false) => {}       // Restricted → proceed
            Err(e) => {
                error!(
                    ?e,
                    "tee-subgroup-admit: open-chain check failed (descendant)"
                );
                continue;
            }
        }
        // Only if we hold this subgroup's key can we admit + deliver it.
        match GroupKeyring::new(store, sub).load_current_key() {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(e) => {
                error!(
                    ?e,
                    "tee-subgroup-admit: load_current_key failed (descendant)"
                );
                continue;
            }
        }
        admit_member_into_subgroup(context_client, store, &sub, &member, &record).await;
    }
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
