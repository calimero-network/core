//! Following what this account gains, and unfollowing what it leaves.
//!
//! The device side of the account namespace: a listener on the op-event channel,
//! beside [`crate::auto_follow`] and shaped like it. Three events, and all three
//! only from THIS node's account namespace - anything on another group's DAG is
//! another account's business.
//!
//! Following is [`crate::handlers::follow_namespace`]: note participation,
//! subscribe, pull. From there the existing machinery converges the namespace - the
//! beacon rescue pulls its DAG, the key pull succeeds because the gainer bound
//! this device, the target folds, bytecode acquisition runs and contexts join
//! through the account's auto-follow flags. Both halves are idempotent, so
//! following one this node already takes part in costs nothing.
//!
//! Unfollowing is the unsubscribe alone. Local state for the namespace is kept,
//! as the holder's own `leave_namespace` keeps it, so a later gain re-follows
//! into state that is already there.

use std::sync::Mutex;

use calimero_account::DeviceId;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::FollowNamespaceRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{
    AccountDeviceRegistry, AccountNamespaceSet, KnownDeviceCert, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Spawn the account-follow handler. Returns immediately; the handler runs as a
/// detached tokio task for the process lifetime.
///
/// Subscribes synchronously before spawning, for the reason `auto_follow::spawn`
/// gives: an event fired between this returning and the task's first poll would
/// otherwise be lost, and the DAG re-drives it only on the next restart.
pub fn spawn(store: Store, node_client: NodeClient, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("account-follow HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("account-follow handler already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        run(rx, store, node_client, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

/// Abort the running handler task. Safe with none running; after it, [`spawn`]
/// may be called again to rebind to a new store or clients. Aborting drops the
/// run task's `JoinSet`, so the follows still in flight are aborted with it.
pub fn shutdown() {
    if let Some(state) = HANDLE
        .lock()
        .expect("account-follow HANDLE poisoned")
        .take()
    {
        state.abort.abort();
    }
}

async fn run(
    mut rx: broadcast::Receiver<OpEvent>,
    store: Store,
    node_client: NodeClient,
    context_client: ContextClient,
) {
    info!("account-follow handler started");

    // Nothing re-drives a gain applied while no listener was up - a lagged recv,
    // or the shutdown-to-spawn window of a restart - so every start sweeps.
    if let Some((account_namespace, own)) = own_registry_scope(&store) {
        for namespace in namespaces_now_covered(&store, account_namespace.to_bytes(), own.device())
        {
            follow(&context_client, namespace).await;
        }
    }

    // Per-event work runs on its own task and this set owns the handles, so
    // aborting the handler aborts the follows still in flight - as `auto_follow`.
    let mut tasks = tokio::task::JoinSet::new();

    loop {
        // Reap finished handlers so the set does not grow across a long run.
        while tasks.try_join_next().is_some() {}

        let event = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "account-follow subscriber lagged; a gain dropped here is followed by \
                     the sweep on this node's next start"
                );
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("account-follow op-event channel closed; handler exiting");
                break;
            }
        };

        // Each event on its own task: a follow waits on an actor round-trip and a
        // subscribe, and a blocked recv loop drops unrelated events as `Lagged`.
        match event {
            OpEvent::AccountNamespaceGained {
                group_id,
                namespace,
                application,
            } => {
                let store = store.clone();
                let context_client = context_client.clone();
                let _ = tasks.spawn(async move {
                    handle_namespace_gained(
                        &store,
                        &context_client,
                        group_id,
                        namespace,
                        application,
                    )
                    .await;
                });
            }
            OpEvent::AccountNamespaceLeft {
                group_id,
                namespace,
            } => {
                let store = store.clone();
                let node_client = node_client.clone();
                let _ = tasks.spawn(async move {
                    handle_namespace_left(&store, &node_client, group_id, namespace).await;
                });
            }
            OpEvent::AccountDeviceCertified { group_id, device } => {
                let store = store.clone();
                let context_client = context_client.clone();
                let _ = tasks.spawn(async move {
                    handle_device_certified(&store, &context_client, group_id, device).await;
                });
            }
            _ => {}
        }
    }
}

/// This node's own row in its account namespace's registry.
///
/// `None` on a node that follows no account namespace, and on one whose own
/// certified op has not been folded here yet - which simply does nothing until
/// it arrives.
fn own_registry_scope(store: &Store) -> Option<(ContextGroupId, KnownDeviceCert)> {
    let devices = NodeDeviceRepository::new(store);
    let resolved = || -> EyreResult<Option<(ContextGroupId, KnownDeviceCert)>> {
        let Some(account_namespace) = devices.account_namespace()? else {
            return Ok(None);
        };
        let Some(held) = devices.get()? else {
            return Ok(None);
        };
        Ok(AccountDeviceRegistry::new(store, account_namespace)
            .device(held.device())?
            .map(|(cert, _epoch)| (account_namespace, cert)))
    };
    match resolved() {
        Ok(found) => found,
        Err(err) => {
            warn!(?err, "account-follow: failed to read this node's own scope");
            None
        }
    }
}

/// Does a gain announced in `group_id`, of a namespace targeting `application`,
/// ask this node to follow it?
pub(crate) fn follows_on_gain(
    store: &Store,
    group_id: [u8; 32],
    application: Option<ApplicationId>,
) -> bool {
    match own_registry_scope(store) {
        Some((account_namespace, own)) => {
            account_namespace.to_bytes() == group_id && own.covers(application)
        }
        None => false,
    }
}

/// The namespaces a freshly certified scope for THIS node's device now covers.
///
/// How a device paired after the account gained its namespaces, or widened
/// later, follows what it now may. Empty for any other device or group.
pub(crate) fn namespaces_now_covered(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
) -> Vec<ContextGroupId> {
    let Some((account_namespace, own)) = own_registry_scope(store) else {
        return Vec::new();
    };
    if account_namespace.to_bytes() != group_id || own.device() != device {
        return Vec::new();
    }
    match AccountNamespaceSet::new(store, account_namespace).namespaces() {
        Ok(set) => set
            .into_iter()
            .filter(|(_, application)| own.covers(*application))
            .map(|(namespace, _)| namespace)
            .collect(),
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read the account's namespace set"
            );
            Vec::new()
        }
    }
}

/// Does a leave announced in `group_id` ask this node to unfollow?
///
/// Deliberately not gated on a registry row: a device unfollows what its account
/// left whether or not its own scope has arrived. Never the account namespace
/// itself, which is the topic every event acted on here arrives over.
pub(crate) fn unfollows_on_left(
    store: &Store,
    group_id: [u8; 32],
    namespace: ContextGroupId,
) -> bool {
    match NodeDeviceRepository::new(store).account_namespace() {
        Ok(Some(account_namespace)) => {
            account_namespace.to_bytes() == group_id && namespace != account_namespace
        }
        Ok(None) => false,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read this node's account namespace"
            );
            false
        }
    }
}

async fn handle_namespace_gained(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    namespace: ContextGroupId,
    application: Option<ApplicationId>,
) {
    if follows_on_gain(store, group_id, application) {
        follow(context_client, namespace).await;
    }
}

async fn handle_device_certified(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    device: DeviceId,
) {
    for namespace in namespaces_now_covered(store, group_id, device) {
        follow(context_client, namespace).await;
    }
}

async fn handle_namespace_left(
    store: &Store,
    node_client: &NodeClient,
    group_id: [u8; 32],
    namespace: ContextGroupId,
) {
    if !unfollows_on_left(store, group_id, namespace) {
        return;
    }
    match node_client
        .unsubscribe_namespace(namespace.to_bytes())
        .await
    {
        Ok(()) => info!(
            ?namespace,
            "account-follow: unfollowed a namespace the account left"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to unfollow a namespace the account left"
        ),
    }
}

async fn follow(context_client: &ContextClient, namespace: ContextGroupId) {
    match context_client
        .follow_namespace(FollowNamespaceRequest { namespace })
        .await
    {
        Ok(()) => info!(
            ?namespace,
            "account-follow: following a namespace the account gained"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to follow a namespace the account gained"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        op_events, AccountDeviceRegistry, AccountNamespaceSet, NamespaceRepository,
        NodeDeviceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use tokio::time::sleep;

    use super::{follows_on_gain, namespaces_now_covered, run, unfollows_on_left};
    use crate::test_support::actor;

    const ACCOUNT_NAMESPACE: [u8; 32] = [0xC1; 32];
    const OTHER_GROUP: [u8; 32] = [0xC9; 32];

    fn ns(seed: u8) -> ContextGroupId {
        ContextGroupId::from([seed; 32])
    }

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// This node as a DEVICE of an account whose root lives elsewhere, scoped to
    /// `applications` (empty is every application): the account namespace
    /// recorded by pairing, this node's device minted under the account's
    /// genesis, and its own certified row folded - the state a device is in once
    /// pairing has settled.
    ///
    /// A device and not a holder on purpose. `NodeDeviceRepository::account_namespace`
    /// answers a holder from its root derivation and only falls through to the
    /// stored row otherwise, so a holder fixture would exercise the wrong read.
    /// The account root comes back beside them because it lives nowhere in this
    /// store, and a caller that has to sign for the account has no other source.
    fn a_device_scoped_to(
        store: &Store,
        applications: &[ApplicationId],
    ) -> (ContextGroupId, DeviceId, PrivateKey) {
        let root_sk = PrivateKey::from([0x70; 32]);
        let genesis = AccountGenesis::new(root_sk.public_key());
        let account = genesis.account_id();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);

        let devices = NodeDeviceRepository::new(store);
        devices
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");
        let held = devices
            .ensure_enrolled_into(&[account_namespace], genesis)
            .expect("mint this node's device");

        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: DeviceCert::sign(
                &root_sk,
                account,
                held.device(),
                &PrivateKey::from([0x71; 32]).public_key(),
                &KemPublicKey::from([0x72; 32]),
                0,
                0,
            )
            .expect("the account root signs this device's certificate"),
        };
        let _recorded = AccountDeviceRegistry::new(store, account_namespace)
            .record(&proof, applications, 0)
            .expect("record this device in its account's registry");
        (account_namespace, held.device(), root_sk)
    }

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// The scope decides. A namespace targeting an application this device may
    /// not speak for is one it must not start replicating.
    #[test]
    fn a_covered_gain_is_followed_and_an_uncovered_one_is_not() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);

        assert!(follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x11))
        ));
        assert!(!follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x22))
        ));
        assert!(
            !follows_on_gain(&store, account_namespace.to_bytes(), None),
            "a namespace whose target has not synced is reachable only by an unscoped device"
        );
    }

    /// A device with no registry row yet does nothing, and catches up when its
    /// own scope arrives.
    #[test]
    fn a_device_that_has_not_folded_its_own_scope_follows_nothing() {
        let store = store();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);
        let devices = NodeDeviceRepository::new(&store);
        devices
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");
        let _held = devices
            .ensure_enrolled_into(
                &[account_namespace],
                AccountGenesis::new(PrivateKey::from([0x70; 32]).public_key()),
            )
            .expect("mint this node's device");

        assert!(!follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x11))
        ));
    }

    /// Somebody else's account namespace is somebody else's business.
    #[test]
    fn an_event_from_another_group_is_ignored() {
        let store = store();
        let (_account_namespace, device, _root_sk) = a_device_scoped_to(&store, &[]);

        assert!(!follows_on_gain(&store, OTHER_GROUP, Some(app(0x11))));
        assert!(!unfollows_on_left(&store, OTHER_GROUP, ns(0x91)));
        assert_eq!(namespaces_now_covered(&store, OTHER_GROUP, device), vec![]);
    }

    /// The catch-up path: a device paired after the account gained its
    /// namespaces, or widened later, follows what it now may and nothing else.
    #[test]
    fn this_nodes_own_scope_arriving_follows_the_set_it_now_covers() {
        let store = store();
        let (account_namespace, device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(ContextGroupId::from([0x91; 32]), Some(app(0x11)))
            .expect("in scope");
        set.record(ContextGroupId::from([0x92; 32]), Some(app(0x22)))
            .expect("out of scope");
        set.record(ContextGroupId::from([0x93; 32]), None)
            .expect("target unknown");

        assert_eq!(
            namespaces_now_covered(&store, account_namespace.to_bytes(), device),
            vec![ContextGroupId::from([0x91; 32])]
        );
        assert_eq!(
            namespaces_now_covered(
                &store,
                account_namespace.to_bytes(),
                DeviceId::from([0xCA; 32])
            ),
            vec![],
            "another device's scope says nothing about what this one may follow"
        );
    }

    /// Nothing re-drives a gain that was applied while no listener was up: the
    /// certified event fires once, an applied op is never re-applied, and a
    /// namespace with no participation row is invisible to the startup sweep. So
    /// the listener sweeps the account's set itself, every time it starts.
    #[actix::test]
    async fn a_gain_applied_while_nothing_listened_is_followed_on_the_next_start() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let missed = ContextGroupId::from([0x94; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(missed, Some(app(0x11)))
            .expect("a gain that landed while this device had no listener");

        let harness = actor::over(store.clone()).await;
        // Beside the one `Actor::started` spawned, because that one is a
        // process-global singleton any other harness in this binary would abort.
        let listener = tokio::spawn(run(
            op_events::subscribe(),
            store.clone(),
            harness.node_client.clone(),
            harness.context_client.clone(),
        ));

        let namespaces = NamespaceRepository::new(&store);
        for _ in 0..100 {
            if namespaces
                .participating_namespaces()
                .expect("read the participation rows")
                .contains(&missed)
            {
                listener.abort();
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("the listener never followed a namespace its account had already gained");
    }

    /// A leave is unfollowed whether or not this device's own scope ever arrived.
    #[test]
    fn a_left_namespace_is_unfollowed_without_a_registry_row() {
        let store = store();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);
        NodeDeviceRepository::new(&store)
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");

        assert!(unfollows_on_left(
            &store,
            account_namespace.to_bytes(),
            ns(0x91)
        ));
    }

    /// A sibling's leave never cuts this device off its own account topic, which
    /// is where every other event it acts on arrives.
    #[test]
    fn the_account_namespace_itself_is_never_unfollowed() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);

        assert!(!unfollows_on_left(
            &store,
            account_namespace.to_bytes(),
            account_namespace
        ));
    }
}
