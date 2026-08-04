//! Enrols this node's account into a namespace as soon as it can, so nobody has
//! to ask.
//!
//! # Why this is not an operator's decision
//!
//! Enrolling has no policy in it. There is no case where a node that has joined a
//! namespace should not have an account there, and every consequence of skipping it
//! is silent misattribution rather than an error: without a device binding, the
//! node's writes attribute to `legacy_account_id(sign_pk)` — a stand-in derived
//! from a bare key, with none of an account's properties — and a writer grant made
//! to its real account never matches. A surface with no decision behind it is one
//! whose only function is to be forgotten, and it was: 3 of 57 e2e scenarios
//! enrolled.
//!
//! # Why the trigger is key delivery and not the join
//!
//! Enrolment publishes `AccountDeviceLinked`, which is an **encrypted** `GroupOp`.
//! Encrypting it needs the namespace scope key, and a joiner does not have one at
//! join time — it arrives afterwards. Enrolling on join therefore deadlocks on the
//! very link that would get it the key, which is why `create_account` refuses
//! outright when the keyring is empty.
//!
//! So the trigger is the key's arrival. `OpEvent::GroupKeyDelivered` already fires
//! for exactly that and is already consumed by `join_group` as its wake-up signal;
//! this listener is a second subscriber, not a new mechanism.
//!
//! # Liveness: the event is not enough
//!
//! A node that received its key while this listener was not running — an older
//! binary, a crash between delivery and enrolment, a restart — has no event coming.
//! So there is also a **startup sweep**, and the useful property here is that it
//! needs no persisted worklist: the condition is derivable from state the node
//! already keeps. "I hold a key for this namespace, I have a namespace identity
//! there, and I hold no device" IS the work item. `rotation_listener` needs a
//! durable `GroupPendingKeyRotation` row for the same reason this does not — what it
//! owes is not otherwise visible.
//!
//! # What makes this delicate
//!
//! Enrolment mints a `DeviceId`, and that id **is** the replica id for this
//! namespace's CRDT state: counter slots and an HLC lineage are held under it.
//! `NodeDeviceRepository::ensure_enrolled` goes to lengths never to mint a second
//! one, because doing so strands everything written under the first. This listener
//! therefore never mints anything itself; it asks the actor, which routes to
//! `ensure_enrolled` — idempotent under a lock, and the single place that decision
//! is made. A duplicated or late request is a no-op, which is what makes reacting
//! to a best-effort broadcast safe.
//!
//! Failures are quiet and retried rather than surfaced: there is no caller to
//! return an error to. Every retry path converges on the same idempotent request, so
//! overlapping attempts are harmless.

use std::collections::BTreeSet;
use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    op_events, op_events::OpEvent, GroupKeyring, MetaRepository, NamespaceRepository,
    NodeDeviceRepository,
};
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Start the enrolment listener. Returns immediately; it runs as a detached task.
///
/// Subscribes to `op_events` **synchronously, before** spawning, so a key delivered
/// between this call and the task starting is not missed — the same race
/// `rotation_listener` and the TEE-admit listener close the same way.
///
/// Idempotent: a second call while one is running is a no-op.
pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("enrol-listener HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("enrolment listener already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        // Sweep first: a node holding a key from before this listener existed has no
        // event coming, and without this it would never enrol at all.
        sweep(&store, &context_client).await;
        run(rx, store, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

/// Abort the listener. For tests and graceful shutdown; safe if none is running.
pub fn shutdown() {
    if let Some(state) = HANDLE
        .lock()
        .expect("enrol-listener HANDLE poisoned")
        .take()
    {
        state.abort.abort();
    }
}

/// Every namespace this node could enrol into but has not.
///
/// Derived rather than recorded, so there is nothing to keep in sync and nothing to
/// migrate. Groups are enumerated and resolved to their namespaces because a key and
/// a device are both namespace-scoped while membership is recorded per group — two
/// subgroups of one namespace must not produce two work items.
///
/// `holds_any_key` deliberately, not "holds the current key": any key at all means
/// the node can encrypt a `GroupOp`, which is the only thing enrolment needs. A node
/// whose key has since rotated is still able to enrol, and refusing it would strand
/// exactly the node most in need of a binding.
fn pending_namespaces(store: &Store) -> Vec<ContextGroupId> {
    let groups = match MetaRepository::new(store).enumerate_all(0, usize::MAX) {
        Ok(groups) => groups,
        Err(err) => {
            warn!(%err, "enrolment sweep: could not enumerate groups");
            return Vec::new();
        }
    };

    let namespaces = NamespaceRepository::new(store);
    let devices = NodeDeviceRepository::new(store);
    let mut seen = BTreeSet::new();
    let mut pending = Vec::new();

    for (group_id, _meta) in groups {
        let group_id = ContextGroupId::from(group_id);
        let Ok(namespace) = namespaces.resolve(&group_id) else {
            continue;
        };
        if !seen.insert(namespace) {
            continue;
        }
        // Already enrolled — the common case, and the reason this is cheap.
        if matches!(devices.get(&namespace), Ok(Some(_))) {
            continue;
        }
        // No key yet: nothing to encrypt the link with. The `GroupKeyDelivered`
        // event will bring us back here.
        if !matches!(
            GroupKeyring::new(store, namespace).holds_any_key(),
            Ok(true)
        ) {
            continue;
        }
        pending.push(namespace);
    }
    pending
}

/// Enrol into every namespace that is ready and not yet enrolled.
async fn sweep(store: &Store, context_client: &ContextClient) {
    let pending = pending_namespaces(store);
    if pending.is_empty() {
        return;
    }
    info!(
        count = pending.len(),
        "enrolment sweep: enrolling into namespaces that hold a scope key but no device"
    );
    for namespace in pending {
        enrol(context_client, namespace).await;
    }
}

async fn run(
    mut rx: tokio::sync::broadcast::Receiver<OpEvent>,
    store: Store,
    context_client: ContextClient,
) {
    info!("enrolment listener started");
    loop {
        let event = match rx.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                // A dropped event is not a dropped enrolment: the condition lives in
                // the store, so the next startup sweep still finds it. It does delay
                // enrolment until then, which is why the sweep is not optional.
                warn!(
                    skipped,
                    "enrolment listener lagged; any missed enrolment is still derivable from \
                     stored state and will be picked up by the next startup sweep"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("enrolment listener: op-event channel closed; exiting");
                break;
            }
        };

        let OpEvent::GroupKeyDelivered { group_id, .. } = event else {
            continue;
        };
        let group_id = ContextGroupId::from(group_id);

        // The event is per-group and the broadcast is process-wide, so resolve to the
        // namespace before deciding anything — a key delivered for a subgroup still
        // means the namespace can now encrypt.
        let Ok(namespace) = NamespaceRepository::new(&store).resolve(&group_id) else {
            continue;
        };
        // Cheap pre-filter. The actor re-checks under the mint lock, which is the
        // authoritative answer; this just avoids a mailbox round-trip per delivered
        // key once the node is enrolled, which is the steady state.
        if matches!(
            NodeDeviceRepository::new(&store).get(&namespace),
            Ok(Some(_))
        ) {
            continue;
        }

        // Own task per event, so a slow publish never blocks the receive loop and
        // lets the bounded broadcast channel overflow. Enrolment is idempotent, so a
        // late or duplicated task is harmless.
        let context_client = context_client.clone();
        tokio::spawn(async move {
            enrol(&context_client, namespace).await;
        });
    }
}

/// Ask the actor to enrol. It re-checks everything that matters — a namespace
/// identity, a scope key, whether a device already exists — and refuses cleanly if
/// this node is not ready, so a speculative call is expected rather than a problem.
async fn enrol(context_client: &ContextClient, namespace_id: ContextGroupId) {
    let request = calimero_context_client::group::CreateAccountRequest { namespace_id };
    match context_client.create_account(request).await {
        Ok(response) => {
            info!(
                ?namespace_id,
                account = %response.account,
                "enrolled this node's account automatically"
            );
        }
        Err(err) => {
            // Debug, not warn, and deliberately: the overwhelmingly common reason to
            // land here is a race this listener is designed to lose — the key arrived
            // but the namespace identity or the keyring is not readable *yet*, or
            // another trigger enrolled first. All of those are retried by the next
            // event or the next startup sweep. Warning on each would make a healthy
            // node look broken every time it joined something.
            debug!(
                ?namespace_id,
                %err,
                "automatic enrolment did not succeed; it will be retried on the next key \
                 delivery or startup sweep"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_governance_store::{GroupKeyring, MetaRepository, NodeDeviceRepository};
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::UpgradePolicy;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{pending_namespaces, ContextGroupId};

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A root group: `NamespaceRepository::resolve` returns a group with no parent
    /// as itself, so writing the meta row is all a namespace needs here.
    fn namespace(store: &Store, id: u8) -> ContextGroupId {
        let group = ContextGroupId::from([id; 32]);
        MetaRepository::new(store)
            .save(
                &group,
                &GroupMetaValue {
                    app_key: [0xAA; 32],
                    target_application_id: ApplicationId::from([0x66; 32]),
                    upgrade_policy: UpgradePolicy::Automatic,
                    created_at: 1_700_000_000,
                    admin_identity: PublicKey::from([0x01; 32]),
                    owner_identity: PublicKey::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("meta row must save");
        group
    }

    /// The whole point of the sweep: work is *derived*, not recorded. A namespace
    /// that holds a scope key and no device is exactly what needs enrolling, and
    /// nothing had to be written down for the node to know that after a restart.
    #[test]
    fn a_namespace_with_a_key_and_no_device_is_pending() {
        let store = store();
        let ns = namespace(&store, 0xA1);
        GroupKeyring::new(&store, ns)
            .store_key(&[0x11; 32])
            .expect("key must store");

        assert_eq!(pending_namespaces(&store), vec![ns]);
    }

    /// No key means nothing can encrypt the `AccountDeviceLinked` op, so enrolling
    /// would fail. Skipping here is what makes `create_account`'s refusal
    /// unreachable via this path rather than merely survivable.
    #[test]
    fn a_namespace_without_a_key_is_not_pending() {
        let store = store();
        let _ns = namespace(&store, 0xA2);

        assert!(pending_namespaces(&store).is_empty());
    }

    /// The steady state, and the reason the sweep is cheap: an enrolled namespace
    /// produces no work on every subsequent start.
    #[test]
    fn an_enrolled_namespace_is_not_pending() {
        let store = store();
        let ns = namespace(&store, 0xA3);
        GroupKeyring::new(&store, ns)
            .store_key(&[0x11; 32])
            .expect("key must store");
        let _device = NodeDeviceRepository::new(&store)
            .ensure_enrolled(&ns)
            .expect("enrolment must succeed");

        assert!(
            pending_namespaces(&store).is_empty(),
            "an already-enrolled namespace must not be swept again — re-minting a \
             device would strand the CRDT state held under the first one"
        );
    }

    /// Each namespace is judged on its own: one being ready must not drag in a
    /// keyless sibling, and one being enrolled must not mask a sibling that is not.
    #[test]
    fn namespaces_are_judged_independently() {
        let store = store();
        let ready = namespace(&store, 0xB1);
        let keyless = namespace(&store, 0xB2);
        let done = namespace(&store, 0xB3);

        for ns in [ready, done] {
            GroupKeyring::new(&store, ns)
                .store_key(&[0x11; 32])
                .expect("key must store");
        }
        let _device = NodeDeviceRepository::new(&store)
            .ensure_enrolled(&done)
            .expect("enrolment must succeed");

        let pending = pending_namespaces(&store);
        assert_eq!(pending, vec![ready], "got {pending:?}");
        assert!(!pending.contains(&keyless));
        assert!(!pending.contains(&done));
    }

    /// A namespace is swept once however many groups resolve to it. Two subgroups
    /// of one namespace share a keyring and a device slot, so counting groups
    /// instead of namespaces would ask the actor to enrol the same node twice.
    #[test]
    fn subgroups_do_not_produce_duplicate_work() {
        use calimero_governance_store::NamespaceRepository;

        let store = store();
        let root = namespace(&store, 0xC1);
        let child = namespace(&store, 0xC2);
        // `nest` is the documented test/legacy helper for a direct parent-edge
        // write; production emits `RootOp::GroupCreated`.
        NamespaceRepository::new(&store)
            .nest(&root, &child)
            .expect("parent link must save");
        GroupKeyring::new(&store, root)
            .store_key(&[0x11; 32])
            .expect("key must store");

        assert_eq!(
            pending_namespaces(&store),
            vec![root],
            "the child must resolve to its root and be deduped, not counted again"
        );
    }
}
