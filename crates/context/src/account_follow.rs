//! Following what this account gains, unfollowing (or purging) what it leaves,
//! binding a sibling it certifies and carrying a revocation it proved: an op-event listener
//! beside [`crate::auto_follow`], reacting only to this node's own account
//! namespace, and sweeping both ways on every start.
//!
//! Unfollowing pulls once before it unsubscribes, since the `MemberLeft` the
//! local view turns on travels on the topic about to be dropped. It needs all
//! three of: folded here, absent from the set, member row gone - an unsynced
//! namespace answers absent to the last two. The sweep covers those two arms
//! alone: a certificate or a revocation folded before the listener was up is
//! repaired by a relink or a repeated revoke, never re-driven.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use actix::Addr;
use calimero_account::{AccountId, DeviceId, SignedDeviceRevocation};
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{
    account_for_group, bind_device_everywhere, replay_known_revocations, withdraw_device_in,
    AccountBindingRepository, AccountDeviceRegistry, AccountNamespaceSet, ForgottenNamespaces,
    KnownDeviceCert, MetaRepository, NamespaceDagService, NamespaceRepository,
    NodeDeviceRepository, PendingNamespacePurges,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio::task::AbortHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::account_namespace::account_is_member;
use crate::handlers::pair_device_complete::{self, signing_identity};
use crate::handlers::purge_namespace::PurgeNamespace;
use crate::ContextManager;

#[cfg(not(test))]
const PROJECTION_JITTER_MAX_MS: u64 = 2_000; // damps k devices publishing one certification's m binds at once
#[cfg(test)]
const PROJECTION_JITTER_MAX_MS: u64 = 5; // the tests assert on the bind, not on the wait

static HANDLE: Mutex<Option<AbortHandle>> = Mutex::new(None);

/// Spawn the account-follow handler, detached for the process lifetime.
///
/// Subscribes before spawning, as `auto_follow::spawn` does: an event fired
/// before the task's first poll would be lost until the next restart.
pub fn spawn(
    store: Store,
    node_client: NodeClient,
    ack_router: Arc<AckRouter>,
    manager: Addr<ContextManager>,
) {
    let mut slot = HANDLE.lock().expect("account-follow HANDLE poisoned");
    if slot.as_ref().is_some_and(|abort| !abort.is_finished()) {
        debug!("account-follow handler already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    *slot = Some(
        tokio::spawn(async move {
            run(rx, store, node_client, ack_router, manager).await;
        })
        .abort_handle(),
    );
}

/// Abort the running handler; [`spawn`] may then rebind it. Aborting drops the
/// run task's `JoinSet`, so the follows still in flight go with it.
pub fn shutdown() {
    if let Some(abort) = HANDLE
        .lock()
        .expect("account-follow HANDLE poisoned")
        .take()
    {
        abort.abort();
    }
}

async fn run(
    mut rx: broadcast::Receiver<OpEvent>,
    store: Store,
    node_client: NodeClient,
    ack_router: Arc<AckRouter>,
    manager: Addr<ContextManager>,
) {
    info!("account-follow handler started");

    // Nothing re-drives a gain, leave or armed purge applied while no listener was up
    // (a lagged recv, a restart's window), so a start sweeps all three.
    for namespace in PendingNamespacePurges::new(&store)
        .armed()
        .unwrap_or_default()
    {
        purge_if_left(&store, &manager, namespace).await;
    }
    for namespace in namespaces_the_account_left(&store) {
        unfollow(&node_client, namespace).await;
    }
    if let Some((account_namespace, own)) = own_registry_scope(&store) {
        let (covered, uncovered) = namespaces_in_scope(&store, account_namespace, &own);
        for namespace in covered {
            follow(&store, &node_client, namespace).await;
        }
        // A device offline while its scope shrank folds the narrowing with no
        // listener up, so the sweep is the only thing that lets those topics go.
        for namespace in uncovered {
            unfollow(&node_client, namespace).await;
        }
    }

    // Per-event work runs on its own task and this set owns the handles, so
    // aborting the handler aborts the follows still in flight - as `auto_follow`.
    let mut tasks = tokio::task::JoinSet::new();
    // Held across a topic decision and its subscribe or unsubscribe, so they
    // land in the order they were decided. A burst of this device's own
    // certificates (a catch-up folding a narrowed scope and the one replacing
    // it) otherwise races: the task that read the old scope can drop a topic
    // after the one that read the new scope kept it, and no later event
    // re-follows it.
    let topics = Arc::new(AsyncMutex::new(()));

    // A replay dropped by a restart or a failed publish is repaired here; each is
    // idempotent, since only a withdrawal still owed is published.
    {
        let store = store.clone();
        let node_client = node_client.clone();
        let ack_router = Arc::clone(&ack_router);
        let _ = tasks.spawn(async move {
            match NamespaceRepository::new(&store).participating_namespaces() {
                Ok(namespaces) => {
                    replay_revocations_into(&store, &node_client, &ack_router, &namespaces).await;
                }
                Err(err) => warn!(
                    ?err,
                    "account-follow: failed to read namespaces to replay into"
                ),
            }
        });
    }

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
                let node_client = node_client.clone();
                let topics = Arc::clone(&topics);
                let _ = tasks.spawn(async move {
                    let _decided = topics.lock().await;
                    // A rejoin ends a purge armed before it.
                    if account_namespace_if_ours(&store, group_id).is_some() {
                        disarm(&store, namespace);
                    }
                    if follows_on_gain(&store, group_id, application) {
                        follow(&store, &node_client, namespace).await;
                    }
                });
            }
            OpEvent::AccountNamespaceLeft {
                group_id,
                namespace,
                purge,
            } => {
                let store = store.clone();
                let node_client = node_client.clone();
                let manager = manager.clone();
                let topics = Arc::clone(&topics);
                let _ = tasks.spawn(async move {
                    let _decided = topics.lock().await;
                    if purge {
                        if unfollows_on_left(&store, group_id, namespace) {
                            purge_if_left(&store, &manager, namespace).await;
                        }
                    } else {
                        // A plain leave withdraws a purge armed before it.
                        if account_namespace_if_ours(&store, group_id).is_some() {
                            disarm(&store, namespace);
                        }
                        handle_namespace_left(&store, &node_client, group_id, namespace).await;
                    }
                });
            }
            // Only this account's own removal finishes a purge; the check just skips the
            // task for everyone else's, since `purge_if_left` re-reads membership.
            OpEvent::MemberRemoved { group_id, member } => {
                if own_account(&store) == Some(member) {
                    let store = store.clone();
                    let manager = manager.clone();
                    let topics = Arc::clone(&topics);
                    let _ = tasks.spawn(async move {
                        let _decided = topics.lock().await;
                        let namespace = ContextGroupId::from(group_id);
                        if PendingNamespacePurges::new(&store)
                            .is_armed(&namespace)
                            .unwrap_or(false)
                        {
                            purge_if_left(&store, &manager, namespace).await;
                        }
                    });
                }
            }
            OpEvent::AccountDeviceCertified { group_id, device } => {
                // Two rules on one event, and they are exclusive by device: this
                // node's own scope arriving is a follow, any other device's is a bind.
                let carry_store = store.clone();
                let carry_node_client = node_client.clone();
                let carry_ack_router = Arc::clone(&ack_router);
                let _ = tasks.spawn(async move {
                    handle_sibling_certified(
                        &carry_store,
                        &carry_node_client,
                        &carry_ack_router,
                        group_id,
                        device,
                    )
                    .await;
                });
                let store = store.clone();
                let node_client = node_client.clone();
                let topics = Arc::clone(&topics);
                let _ = tasks.spawn(async move {
                    // Read under the lock: whichever task runs last reads the
                    // newest scope, so the topics end up matching it.
                    let _decided = topics.lock().await;
                    let (covered, uncovered) =
                        namespaces_this_scope_decides(&store, group_id, device);
                    for namespace in covered {
                        follow(&store, &node_client, namespace).await;
                    }
                    for namespace in uncovered {
                        unfollow(&node_client, namespace).await;
                    }
                });
            }
            // Only a proof-bearing revocation carries: an admin one authorises
            // nothing outside the group whose admin gate let it through.
            OpEvent::DeviceRevoked {
                group_id,
                account,
                device,
                proof: Some(proof),
            } => {
                let store = store.clone();
                let node_client = node_client.clone();
                let ack_router = Arc::clone(&ack_router);
                let _ = tasks.spawn(async move {
                    handle_revocation_carry(
                        &store,
                        &node_client,
                        &ack_router,
                        group_id,
                        account,
                        device,
                        *proof,
                    )
                    .await;
                });
            }
            _ => {}
        }
    }
}

/// Does this node's own certified scope still reach `group`? The one answer the
/// namespace listing, the application report and the authoring gate share.
///
/// Keyed on the registry row, not on a binding: the narrowing is recorded in the
/// account namespace, while the descope travels on the topic being dropped.
///
/// # Errors
/// Propagates the device, namespace and metadata reads.
pub fn node_reaches(store: &Store, group: &ContextGroupId) -> EyreResult<bool> {
    let devices = NodeDeviceRepository::new(store);
    if devices.holder_root()?.is_some() {
        return Ok(true);
    }
    let (Some(account_namespace), Some(held)) = (devices.account_namespace()?, devices.get()?)
    else {
        return Ok(true);
    };
    let namespace = NamespaceRepository::new(store).resolve(group)?;
    if namespace == account_namespace {
        return Ok(true);
    }
    let Some(own) = AccountDeviceRegistry::new(store, account_namespace).device(held.device())?
    else {
        return Ok(true);
    };
    Ok(own.covers(
        MetaRepository::new(store)
            .load(&namespace)?
            .map(|meta| meta.target.application_id),
    ))
}

/// The namespaces this node takes part in that its own scope still reaches.
/// Unfiltered, a start-up sweep races this listener's own unfollow and may undo it.
///
/// # Errors
/// Propagates the participation read and the reads behind [`node_reaches`].
pub fn namespaces_in_reach(store: &Store) -> EyreResult<Vec<ContextGroupId>> {
    let mut reached = Vec::new();
    let forgotten = ForgottenNamespaces::new(store);
    for namespace in NamespaceRepository::new(store).participating_namespaces()? {
        if forgotten.is_forgotten(&namespace)? {
            debug!(
                ?namespace,
                "account-follow: removed from this device; not subscribing"
            );
        } else if node_reaches(store, &namespace)? {
            reached.push(namespace);
        } else {
            debug!(?namespace, "account-follow: this device's scope no longer reaches this namespace; not subscribing");
        }
    }
    Ok(reached)
}

/// The authoring half of [`node_reaches`]: refuse a write no peer would take,
/// rather than answering locally and publishing it.
///
/// # Errors
/// [`crate::error::ContextError::DeviceOutOfScope`], or the reads behind it.
pub fn require_reach(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    if node_reaches(store, group)? {
        return Ok(());
    }
    // Hex, not the id's `Debug`: this message is read by a person.
    Err(crate::error::ContextError::DeviceOutOfScope {
        group_id: hex::encode(group.to_bytes()),
    }
    .into())
}

/// This node's own row in its account namespace's registry. `None` until this
/// node's own certified op is folded here, which does nothing until it arrives.
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
            .map(|cert| (account_namespace, cert)))
    };
    match resolved() {
        Ok(found) => found,
        Err(err) => {
            warn!(?err, "account-follow: failed to read this node's own scope");
            None
        }
    }
}

/// This node's account namespace, when `group_id` is it. The same op folded on a
/// project's DAG is another account's business, or this node's own publish back.
fn account_namespace_if_ours(store: &Store, group_id: [u8; 32]) -> Option<ContextGroupId> {
    match NodeDeviceRepository::new(store).account_namespace() {
        Ok(Some(account_namespace)) if account_namespace.to_bytes() == group_id => {
            Some(account_namespace)
        }
        Ok(_) => None,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read this node's account namespace"
            );
            None
        }
    }
}

/// Has this node folded `namespace`'s governance at all? Without a head, an
/// absent set row and an absent member row are silence rather than a verdict.
fn folded_here(store: &Store, namespace: ContextGroupId) -> EyreResult<bool> {
    Ok(
        !NamespaceDagService::new(store, namespace.to_bytes().into())
            .read_head_record()?
            .parent_hashes
            .is_empty(),
    )
}

/// The namespaces this node takes part in that its account has left: all three
/// reads must say so, since an unsynced namespace and set both read absent.
fn namespaces_the_account_left(store: &Store) -> Vec<ContextGroupId> {
    let devices = NodeDeviceRepository::new(store);
    let resolved = || -> EyreResult<Vec<ContextGroupId>> {
        let (Some(account_namespace), Some(_held)) = (devices.account_namespace()?, devices.get()?)
        else {
            return Ok(Vec::new());
        };
        let set = AccountNamespaceSet::new(store, account_namespace);
        let mut left = Vec::new();
        for namespace in NamespaceRepository::new(store).participating_namespaces()? {
            if namespace == account_namespace {
                continue;
            }
            let dropped = || -> EyreResult<bool> {
                Ok(folded_here(store, namespace)?
                    && set.contains(namespace)?.is_none()
                    && !account_is_member(store, namespace)?)
            };
            match dropped() {
                Ok(true) => left.push(namespace),
                Ok(false) => {}
                // Skipped, not fatal: a namespace whose reads fail is one this
                // sweep knows nothing about, and the next start asks again.
                Err(err) => warn!(
                    ?err,
                    ?namespace,
                    "account-follow: failed to decide whether the account left a namespace"
                ),
            }
        }
        Ok(left)
    };
    match resolved() {
        Ok(left) => left,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read what this account has left"
            );
            Vec::new()
        }
    }
}

/// Does a gain announced in `group_id`, of a namespace targeting `application`,
/// ask this node to follow it?
fn follows_on_gain(store: &Store, group_id: [u8; 32], application: Option<ApplicationId>) -> bool {
    match own_registry_scope(store) {
        Some((account_namespace, own)) => {
            account_namespace.to_bytes() == group_id && own.covers(application)
        }
        None => false,
    }
}

/// What a freshly certified scope for THIS node's device decides: what it now
/// covers, and what it has stopped covering. Both empty for any other device.
fn namespaces_this_scope_decides(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
) -> (Vec<ContextGroupId>, Vec<ContextGroupId>) {
    let Some((account_namespace, own)) = own_registry_scope(store) else {
        return (Vec::new(), Vec::new());
    };
    if account_namespace.to_bytes() != group_id || own.device() != device {
        return (Vec::new(), Vec::new());
    }
    namespaces_in_scope(store, account_namespace, &own)
}

/// The account's namespace set, split by whether `own`'s scope covers each. The
/// account namespace is never in the set, so the uncovered half cannot name it.
fn namespaces_in_scope(
    store: &Store,
    account_namespace: ContextGroupId,
    own: &KnownDeviceCert,
) -> (Vec<ContextGroupId>, Vec<ContextGroupId>) {
    let set = match AccountNamespaceSet::new(store, account_namespace).namespaces() {
        Ok(set) => set,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read the account's namespace set"
            );
            return (Vec::new(), Vec::new());
        }
    };
    // A root holder reaches everywhere, so no statement makes it let a topic go.
    // A failed read counts as holding: guessing wrong costs every topic.
    let holds_the_root = !matches!(NodeDeviceRepository::new(store).holder_root(), Ok(None));
    let (mut covered, mut uncovered) = (Vec::new(), Vec::new());
    for (namespace, application) in set {
        if holds_the_root || own.covers(application) {
            covered.push(namespace);
        } else {
            uncovered.push(namespace);
        }
    }
    (covered, uncovered)
}

/// The certificate to publish for a newly certified device of this account, and
/// the namespaces this node has to publish it into.
///
/// `None` for this node's own device, published by the holder that signed it,
/// and for one revoked ANYWHERE this node takes part - the id is spent
/// everywhere. Never the account namespace, whose unset target no scope names.
fn namespaces_to_bind_into(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
) -> Option<(KnownDeviceCert, Vec<ContextGroupId>)> {
    let account_namespace = account_namespace_if_ours(store, group_id)?;
    let devices = NodeDeviceRepository::new(store);
    match devices.get() {
        Ok(Some(held)) if held.device() == device => return None,
        Ok(_) => {}
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read this node's own device");
            return None;
        }
    }
    match devices.revoked_in(device) {
        Ok(revoked) if !revoked.is_empty() => return None,
        Ok(_) => {}
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read where a sibling is revoked");
            return None;
        }
    }
    let cert = match AccountDeviceRegistry::new(store, account_namespace).device(device) {
        Ok(Some(cert)) => cert,
        Ok(None) => return None,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read a sibling's certificate");
            return None;
        }
    };
    let targets = match pair_device_complete::namespaces_in_scope(store, cert.applications()) {
        Ok(targets) => targets,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to resolve a sibling's scope");
            return None;
        }
    };
    let targets: Vec<_> = targets
        .into_iter()
        .filter(|namespace| *namespace != account_namespace)
        .collect();
    (!targets.is_empty()).then_some((cert, targets))
}

/// Where a proof-bearing revocation folded here is carried: every namespace this
/// node takes part in that has not withdrawn the device, bound there or not.
fn namespaces_to_revoke_in(
    store: &Store,
    group_id: [u8; 32],
    account: AccountId,
    device: DeviceId,
) -> Vec<ContextGroupId> {
    let Some(account_namespace) = account_namespace_if_ours(store, group_id) else {
        return Vec::new();
    };
    match account_for_group(store, &account_namespace) {
        Ok(own) if own == account => {}
        Ok(_) => {
            warn!(%account, %device, "account-follow: not carrying another account's revocation");
            return Vec::new();
        }
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read which account this node speaks for"
            );
            return Vec::new();
        }
    }
    let namespaces = match NamespaceRepository::new(store).participating_namespaces() {
        Ok(namespaces) => namespaces,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read the namespaces this node takes part in"
            );
            return Vec::new();
        }
    };
    let bindings = AccountBindingRepository::new(store);
    let mut targets = Vec::new();
    for namespace in namespaces {
        if namespace == account_namespace {
            continue;
        }
        match bindings.withdrawal_owed(&namespace, account, device) {
            Ok(true) => targets.push(namespace),
            Ok(false) => {}
            // Skipped, not fatal: the other namespaces still get the carry.
            Err(err) => warn!(?err, ?namespace, %device,
                              "account-follow: failed to read whether a device is withdrawn"),
        }
    }
    targets
}

/// Does a leave announced in `group_id` ask this node to unfollow?
///
/// Deliberately not gated on a registry row: a device unfollows what its account
/// left whether or not its own scope has arrived. Never the account namespace
/// itself, which is the topic every event acted on here arrives over.
fn unfollows_on_left(store: &Store, group_id: [u8; 32], namespace: ContextGroupId) -> bool {
    account_namespace_if_ours(store, group_id)
        .is_some_and(|account_namespace| namespace != account_namespace)
}

/// Bind a newly certified sibling where its scope reaches. A projector that is no
/// admin has its `KeyDelivery` refused; the sibling pulls the key for itself.
async fn handle_sibling_certified(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    group_id: [u8; 32],
    device: DeviceId,
) {
    let Some((cert, targets)) = namespaces_to_bind_into(store, group_id, device) else {
        return;
    };
    let signer_sk = match signing_identity(store, &targets) {
        Ok(secret) => PrivateKey::from(secret),
        Err(err) => {
            warn!(?err, %device, "account-follow: no identity to publish a sibling's link with");
            return;
        }
    };
    publish_sibling_link(store, node_client, ack_router, &targets, &signer_sk, &cert).await;
}

async fn publish_sibling_link(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    targets: &[ContextGroupId],
    signer_sk: &PrivateKey,
    cert: &KnownDeviceCert,
) {
    let device = cert.device();
    sleep(Duration::from_millis(rand::random_range(
        0..=PROJECTION_JITTER_MAX_MS,
    )))
    .await;
    // The one rule the per-namespace bind does not repeat, and a revocation can
    // have folded during the wait: a spent id belongs in no namespace at all.
    match NodeDeviceRepository::new(store).revoked_in(device) {
        Ok(revoked) if revoked.is_empty() => {}
        Ok(_) => return,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to re-read where a sibling is revoked");
            return;
        }
    }
    let outcomes =
        bind_device_everywhere(store, node_client, ack_router, targets, signer_sk, cert).await;
    debug!(
        %device,
        ?outcomes,
        "account-follow: carried a newly certified device of this account into the \
         namespaces this node takes part in"
    );
}

/// The carry terminates: it publishes only into non-account namespaces, and the
/// `DeviceRevoked` re-emitted there fails `account_namespace_if_ours`.
async fn handle_revocation_carry(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    group_id: [u8; 32],
    account: AccountId,
    device: DeviceId,
    proof: SignedDeviceRevocation,
) {
    let targets = namespaces_to_revoke_in(store, group_id, account, device);
    if targets.is_empty() {
        return;
    }
    let signer_sk = match signing_identity(store, &targets) {
        Ok(secret) => PrivateKey::from(secret),
        Err(err) => {
            warn!(?err, %device, "account-follow: no identity to carry a revocation with");
            return;
        }
    };
    let op = GroupOp::AccountDeviceUnlinked {
        account,
        device,
        proof: Some(proof),
    };
    debug!(
        %device,
        %account,
        namespaces = targets.len(),
        "account-follow: carrying a revocation into the namespaces the revoker could not reach"
    );
    carry_into(
        store,
        node_client,
        ack_router,
        &targets,
        &signer_sk,
        account,
        device,
        &op,
    )
    .await;
}

/// Publish `op` into each target, skipping any that has withdrawn the device since
/// the decision above was taken: a faster sibling of this account carried it there.
#[expect(
    clippy::too_many_arguments,
    reason = "the publish context plus the one withdrawal it carries"
)]
async fn carry_into(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    targets: &[ContextGroupId],
    signer_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    op: &GroupOp,
) {
    let bindings = AccountBindingRepository::new(store);
    for namespace in targets {
        match bindings.withdrawal_owed(namespace, account, device) {
            Ok(true) => {}
            Ok(false) => continue,
            // Skipped: an unread binding may be another account's, which the publish could spend.
            Err(err) => {
                warn!(?err, ?namespace, %device,
                      "account-follow: failed to re-read a carried device's binding; skipping");
                continue;
            }
        }
        if let Err(err) = withdraw_device_in(
            store,
            node_client,
            ack_router,
            namespace,
            signer_sk,
            device,
            op.clone(),
        )
        .await
        {
            warn!(
                ?err,
                ?namespace,
                %device,
                "account-follow: a namespace did not take the carried revocation; the rest continue"
            );
        }
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
    // Pull it once before dropping the topic: this node's view of the namespace
    // turns on folding its `MemberLeft`, which travels there and not here.
    if let Err(err) = node_client.sync_namespace(namespace.to_bytes()).await {
        warn!(
            ?err,
            ?namespace,
            "account-follow: failed to pull a namespace before unfollowing it"
        );
    }
    unfollow(node_client, namespace).await;
}

/// Withdraw a purge waiting for `namespace`'s leave.
fn disarm(store: &Store, namespace: ContextGroupId) {
    if let Err(err) = PendingNamespacePurges::new(store).disarm(&namespace) {
        warn!(
            ?err,
            ?namespace,
            "account-follow: could not withdraw a pending purge"
        );
    }
}

fn own_account(store: &Store) -> Option<AccountId> {
    NodeDeviceRepository::new(store)
        .get()
        .ok()
        .flatten()
        .map(|held| held.account)
}

/// Purge `namespace` once this account has left it, else arm the purge durably for
/// its `MemberRemoved` or the next start; under the `topics` lock, so no event slips in.
async fn purge_if_left(store: &Store, manager: &Addr<ContextManager>, namespace: ContextGroupId) {
    let pending = PendingNamespacePurges::new(store);
    match account_is_member(store, namespace) {
        Ok(false) => {}
        Ok(true) => {
            if let Err(err) = pending.arm(&namespace) {
                warn!(
                    ?err,
                    ?namespace,
                    "account-follow: could not remember a purge"
                );
            }
            warn!(
                ?namespace,
                "account-follow: a purge came before the account's leave; waiting for it"
            );
            return;
        }
        Err(err) => {
            warn!(
                ?err,
                ?namespace,
                "account-follow: could not tell whether the account left; not purging"
            );
            return;
        }
    }
    match manager.send(PurgeNamespace { namespace }).await {
        Ok(Ok(())) => {
            info!(
                ?namespace,
                "account-follow: purged a namespace the account left"
            );
            if let Err(err) = pending.disarm(&namespace) {
                warn!(
                    ?err,
                    ?namespace,
                    "account-follow: could not clear a finished purge"
                );
            }
        }
        Ok(Err(err)) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to purge a namespace"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: the purge was not delivered"
        ),
    }
}

/// Drop `namespace`'s topic.
async fn unfollow(node_client: &NodeClient, namespace: ContextGroupId) {
    match node_client
        .unsubscribe_namespace(namespace.to_bytes())
        .await
    {
        Ok(()) => info!(?namespace, "account-follow: unfollowed a namespace"),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to unfollow a namespace"
        ),
    }
}

/// Publish this account's recorded withdrawals into each of `namespaces`.
async fn replay_revocations_into(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespaces: &[ContextGroupId],
) {
    if namespaces.is_empty() {
        return;
    }
    let signer_sk = match signing_identity(store, namespaces) {
        Ok(secret) => PrivateKey::from(secret),
        Err(err) => {
            warn!(
                ?err,
                "account-follow: no identity to replay this account's withdrawals with"
            );
            return;
        }
    };
    for namespace in namespaces {
        replay_known_revocations(store, node_client, ack_router, namespace, &signer_sk).await;
    }
}

async fn follow(store: &Store, node_client: &NodeClient, namespace: ContextGroupId) {
    match crate::handlers::follow_namespace::follow(store, node_client, &namespace).await {
        Ok(_identity) => info!(
            ?namespace,
            "account-follow: following a namespace the account gained"
        ),
        Err(err) if is_forgotten(&err) => debug!(
            ?namespace,
            "account-follow: not following a namespace removed from this device"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to follow a namespace the account gained"
        ),
    }
}

fn is_forgotten(err: &eyre::Report) -> bool {
    matches!(
        err.downcast_ref::<crate::error::ContextError>(),
        Some(crate::error::ContextError::NamespaceForgotten { .. })
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{
        AccountGenesis, AccountId, AccountProof, DeviceCert, DeviceId, DeviceRevocation,
        KemPublicKey, SignedDeviceRevocation,
    };
    use calimero_context_client::local_governance::GroupOp;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::op_events::OpEvent;
    use calimero_governance_store::{
        op_events, AccountBindingRepository, AccountDeviceRegistry, AccountNamespaceSet,
        ForgottenNamespaces, GroupKeyring, MembershipRepository, MetaRepository,
        NamespaceRepository, NodeDeviceRepository, PendingNamespacePurges,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};
    use calimero_store::Store;

    use super::{
        carry_into, follows_on_gain, namespaces_in_reach, namespaces_this_scope_decides,
        namespaces_to_bind_into, namespaces_to_revoke_in, node_reaches, publish_sibling_link, run,
        signing_identity, unfollows_on_left,
    };
    use crate::test_support::{
        actor, device_scope, eventually, holder_device_scoped_to, rescope_paired_device,
    };

    const ACCOUNT_NAMESPACE: [u8; 32] = [0xC1; 32];
    const OTHER_GROUP: [u8; 32] = [0xC9; 32];

    fn ns(seed: u8) -> ContextGroupId {
        ContextGroupId::from([seed; 32])
    }

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// This node as a DEVICE of an account whose root lives elsewhere, scoped to
    /// `applications`, with its account namespace fixed at `ACCOUNT_NAMESPACE`.
    fn a_device_scoped_to(
        store: &Store,
        applications: &[ApplicationId],
    ) -> (ContextGroupId, DeviceId, PrivateKey) {
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);
        let (device, root_sk) =
            crate::test_support::paired_device_scoped_to(store, &account_namespace, applications);
        (account_namespace, device, root_sk)
    }

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A namespace this node takes part in, targeting `application`, as a
    /// founder leaves it: a meta row and a participation row.
    fn a_namespace_targeting(store: &Store, namespace: ContextGroupId, application: ApplicationId) {
        MetaRepository::new(store)
            .save(
                &namespace,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: application,
                        ..GroupTarget::default()
                    },
                    created_at: 1_700_000_000,
                    admin_identity: AccountId::from([0x01; 32]),
                    owner_identity: AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
        let _identity = NamespaceRepository::new(store)
            .participate_in(&namespace)
            .expect("take part in the namespace");
    }

    /// A sibling of this account in the registry, scoped to `applications`. Takes
    /// the root key: a device holds none to sign a sibling's certificate with.
    fn a_sibling_scoped_to(
        store: &Store,
        account_namespace: ContextGroupId,
        root_sk: &PrivateKey,
        seed: u8,
        applications: &[ApplicationId],
    ) -> DeviceId {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let device = DeviceId::mint(genesis.account_id(), [seed; 16]);
        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: DeviceCert::sign(
                root_sk,
                genesis.account_id(),
                device,
                &PrivateKey::from([seed; 32]).public_key(),
                &KemPublicKey::from([seed ^ 0xFF; 32]),
                0,
                0,
            )
            .expect("the account root signs a sibling's cert"),
        };
        let _recorded = AccountDeviceRegistry::new(store, account_namespace)
            .record(
                &proof,
                &device_scope(root_sk, &proof.statement, applications, 0),
            )
            .expect("record the sibling");
        device
    }

    /// Still bound there is the whole defect: the descope travels on the topic the
    /// narrowing drops, so the binding can outlive the scope indefinitely.
    #[test]
    fn a_narrowing_stops_a_device_reaching_a_namespace_it_is_still_bound_in() {
        let store = store();
        let (account_namespace, device, root_sk) = a_device_scoped_to(&store, &[]);
        let lost = ns(0xD1);
        a_namespace_targeting(&store, lost, app(0x22));
        let (_ns, own_pk, _sk) = NamespaceRepository::new(&store)
            .participate_in(&lost)
            .expect("this node's identity there");
        let _account = crate::test_support::enrol(&store, &lost, &own_pk);

        assert!(
            node_reaches(&store, &lost).expect("read the scope"),
            "a bound device reaches a namespace its scope covers"
        );

        rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app(0x11)],
            1,
        );

        assert!(
            AccountBindingRepository::new(&store)
                .binding_for_sign_pk(&lost, &own_pk)
                .expect("read the bindings")
                .is_some(),
            "the fixture must keep the binding, or the assertion below proves nothing"
        );
        assert!(
            !node_reaches(&store, &lost).expect("read the scope"),
            "a narrowed device must stop reaching the namespace its account took away"
        );

        rescope_paired_device(&store, &account_namespace, device, &root_sk, &[], 2);

        assert!(
            node_reaches(&store, &lost).expect("read the scope"),
            "a later statement at a wider scope reaches it again"
        );
    }

    /// The sweep and this listener's own unfollow run in different actors, so an
    /// unfiltered sweep races it - and a restart is exactly when both run.
    #[test]
    fn the_startup_sweep_skips_a_namespace_this_scope_no_longer_covers() {
        let store = store();
        let (account_namespace, device, root_sk) = a_device_scoped_to(&store, &[]);
        let kept = ns(0xD5);
        let lost = ns(0xD6);
        a_namespace_targeting(&store, kept, app(0x11));
        a_namespace_targeting(&store, lost, app(0x22));

        let reached = namespaces_in_reach(&store).expect("read the participating set");
        assert!(
            reached.contains(&kept) && reached.contains(&lost),
            "a device scoped to everything sweeps both: {reached:?}"
        );

        rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app(0x11)],
            1,
        );

        let reached = namespaces_in_reach(&store).expect("read the participating set");
        assert!(
            reached.contains(&kept),
            "the covered namespace is still subscribed: {reached:?}"
        );
        assert!(
            !reached.contains(&lost),
            "the uncovered one must not be, or the sweep undoes the narrowing: {reached:?}"
        );
        assert!(
            reached.contains(&account_namespace),
            "the account namespace is reached by every device, narrowed or not"
        );
    }

    /// The device holds a namespace identity and a participation row either way,
    /// so nothing below the scope gate refuses the write.
    #[actix::test]
    async fn a_narrowed_device_is_refused_when_it_authors_where_it_no_longer_reaches() {
        let store = store();
        let (account_namespace, device, root_sk) = a_device_scoped_to(&store, &[]);
        let lost = ns(0xD3);
        a_namespace_targeting(&store, lost, app(0x22));
        let harness = actor::over(store.clone()).await;
        let write = || {
            harness
                .manager
                .send(calimero_context_client::group::SetGroupMetadataRequest {
                    group_id: lost,
                    name: Some("named by a device".to_owned()),
                    data: std::collections::BTreeMap::new(),
                })
        };

        let before = write()
            .await
            .expect("the manager answers")
            .expect_err("this fixture grants no metadata capability either way");
        assert!(
            !matches!(
                before.downcast_ref::<crate::error::ContextError>(),
                Some(crate::error::ContextError::DeviceOutOfScope { .. })
            ),
            "the gate must not fire while the scope still reaches here: {before:?}"
        );

        rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app(0x11)],
            1,
        );

        let refused = write()
            .await
            .expect("the manager answers")
            .expect_err("a narrowed device must not author here");
        assert!(
            matches!(
                refused.downcast_ref::<crate::error::ContextError>(),
                Some(crate::error::ContextError::DeviceOutOfScope { .. })
            ),
            "the refusal must be the typed one a caller can map to a 403: {refused:?}"
        );
        assert!(
            refused.to_string().contains(&hex::encode(lost.to_bytes())),
            "a person reads this message, so it names the namespace in hex: {refused}"
        );
    }

    /// The two nodes that hold their own root: the account holder, and an ordinary
    /// member that joined by invitation. Neither is scoped by anybody.
    #[test]
    fn a_node_holding_its_own_root_reaches_everywhere() {
        let store = store();
        let project = ns(0xD2);
        a_namespace_targeting(&store, project, app(0x22));
        let _sibling = crate::test_support::certify_device(&store, 0x31, &[app(0x11)]);

        assert!(
            node_reaches(&store, &project).expect("read the scope"),
            "a holder's own registry rows scope its siblings, never itself"
        );
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
        assert_eq!(
            namespaces_this_scope_decides(&store, OTHER_GROUP, device),
            (vec![], vec![])
        );
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
            namespaces_this_scope_decides(&store, account_namespace.to_bytes(), device),
            (
                vec![ContextGroupId::from([0x91; 32])],
                vec![
                    ContextGroupId::from([0x92; 32]),
                    ContextGroupId::from([0x93; 32])
                ]
            ),
            "and the set it no longer covers is what it has to let go of"
        );
        assert_eq!(
            namespaces_this_scope_decides(
                &store,
                account_namespace.to_bytes(),
                DeviceId::from([0xCA; 32])
            ),
            (vec![], vec![]),
            "another device's scope says nothing about what this one may follow"
        );
    }

    /// The root holder signs every scope statement, so one naming itself can never
    /// make it let a topic go, whatever it says and however it arrived.
    #[test]
    fn a_root_holders_own_scope_arriving_unfollows_nothing() {
        let store = store();
        let (account_namespace, device) = holder_device_scoped_to(&store, &[app(0x11)]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(ContextGroupId::from([0x91; 32]), Some(app(0x11)))
            .expect("named by the scope");
        set.record(ContextGroupId::from([0x92; 32]), Some(app(0x22)))
            .expect("not named by the scope");

        let (covered, uncovered) =
            namespaces_this_scope_decides(&store, account_namespace.to_bytes(), device);

        assert_eq!(
            uncovered,
            Vec::<ContextGroupId>::new(),
            "the root holder must never unfollow on its own statement"
        );
        assert_eq!(
            covered,
            vec![
                ContextGroupId::from([0x91; 32]),
                ContextGroupId::from([0x92; 32])
            ],
            "and it goes on reaching every namespace of its account"
        );
    }

    /// The same rule at start-up, where no event is involved: a narrower row
    /// folded while this node was down must not cost the holder its topics.
    #[actix::test]
    async fn the_start_up_sweep_drops_nothing_on_a_node_holding_the_account_root() {
        let store = store();
        let (account_namespace, _device) = holder_device_scoped_to(&store, &[app(0x11)]);
        let uncovered = ContextGroupId::from([0x93; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(uncovered, Some(app(0x22)))
            .expect("a namespace the row's scope does not name");

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let mut subscribed = Vec::new();
        let _followed = eventually(|| {
            subscribed.append(&mut harness.subscribed());
            subscribed.contains(&topic(uncovered))
        })
        .await;
        let seen = harness.unsubscribed();
        listener.abort();

        assert!(
            subscribed.contains(&topic(uncovered)),
            "the holder has to follow every namespace of its account; got: {subscribed:?}"
        );
        assert!(
            !seen.contains(&topic(uncovered)),
            "the sweep must not unfollow on the holder's own row; got: {seen:?}"
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
        let listener = listen(&store, &harness);

        let namespaces = NamespaceRepository::new(&store);
        let followed = eventually(|| {
            namespaces
                .participating_namespaces()
                .expect("read the participation rows")
                .contains(&missed)
        })
        .await;
        listener.abort();

        assert!(
            followed,
            "the listener never followed a namespace its account had already gained"
        );
    }

    fn forget(store: &Store, namespace: ContextGroupId) {
        ForgottenNamespaces::new(store)
            .mark(&namespace, None, 1, None)
            .expect("record the removal");
    }

    /// The start-up subscribe sweep and `join_context`'s namespace sync both run
    /// their set through here.
    #[test]
    fn a_removed_namespace_is_left_out_of_the_set_a_device_subscribes() {
        let store = store();
        let (_account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let kept = ns(0xD7);
        let removed = ns(0xD8);
        a_namespace_targeting(&store, kept, app(0x11));
        a_namespace_targeting(&store, removed, app(0x11));
        forget(&store, removed);

        let reached = namespaces_in_reach(&store).expect("read the participating set");

        assert!(
            reached.contains(&kept),
            "the control is reached: {reached:?}"
        );
        assert!(
            !reached.contains(&removed),
            "a removed namespace must not be subscribed again: {reached:?}"
        );
    }

    #[actix::test]
    async fn the_start_up_sweep_does_not_follow_a_removed_namespace() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let kept = ContextGroupId::from([0x96; 32]);
        let removed = ContextGroupId::from([0x97; 32]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(kept, Some(app(0x11))).expect("the control");
        set.record(removed, Some(app(0x11)))
            .expect("the removed one");
        forget(&store, removed);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let mut subscribed = Vec::new();
        let _swept = eventually(|| {
            subscribed.append(&mut harness.subscribed());
            subscribed.contains(&topic(kept))
        })
        .await;
        listener.abort();

        assert!(
            subscribed.contains(&topic(kept)),
            "the sweep follows the control; got: {subscribed:?}"
        );
        assert!(
            !subscribed.contains(&topic(removed)),
            "the sweep must not bring a removed namespace back; got: {subscribed:?}"
        );
        assert!(
            !NamespaceRepository::new(&store)
                .participating_namespaces()
                .expect("read the participation rows")
                .contains(&removed),
            "nor note participation in it"
        );
    }

    #[actix::test]
    async fn a_gain_of_a_removed_namespace_is_not_followed() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let kept = ContextGroupId::from([0x98; 32]);
        let removed = ContextGroupId::from([0x99; 32]);
        forget(&store, removed);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        for namespace in [removed, kept] {
            op_events::notify(OpEvent::AccountNamespaceGained {
                group_id: account_namespace.to_bytes(),
                namespace,
                application: Some(app(0x11)),
            });
        }
        let mut subscribed = Vec::new();
        let _followed = eventually(|| {
            subscribed.append(&mut harness.subscribed());
            subscribed.contains(&topic(kept))
        })
        .await;
        listener.abort();

        assert!(
            subscribed.contains(&topic(kept)),
            "the control gain is followed; got: {subscribed:?}"
        );
        assert!(
            !subscribed.contains(&topic(removed)),
            "a gain of a removed namespace must not follow it; got: {subscribed:?}"
        );
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

    /// A listener of this test's own, beside the one `Actor::started` spawned:
    /// that one is a process-global singleton another harness may have aborted.
    fn listen(store: &Store, harness: &actor::Harness) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run(
            op_events::subscribe(),
            store.clone(),
            harness.node_client.clone(),
            Arc::clone(harness.context_client.ack_router()),
            harness.manager.clone(),
        ))
    }

    fn topic(namespace: ContextGroupId) -> String {
        format!("ns/{}", hex::encode(namespace.to_bytes()))
    }

    /// This node has folded `namespace`'s governance, so its absent set row and
    /// absent member row are verdicts rather than the silence of an unsynced one.
    fn folded(store: &Store, namespace: ContextGroupId) {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::NamespaceGovHead::new(namespace.to_bytes()),
                &calimero_store::key::NamespaceGovHeadValue {
                    sequence: 1,
                    dag_heads: vec![[0x01; 32]],
                },
            )
            .expect("record a governance head for it");
    }

    /// Poll until `done` holds over every unsubscribe seen so far, and hand back
    /// what was seen. The recorder drains on read, so the accumulation is here.
    async fn unsubscribes_until(
        harness: &mut actor::Harness,
        done: impl Fn(&[String]) -> bool,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        let _reached = eventually(|| {
            seen.append(&mut harness.unsubscribed());
            done(&seen)
        })
        .await;
        seen
    }

    /// Unsubscribing IS unfollowing, so the arm is pinned on the topic the node
    /// client dropped rather than on the decision that reached it.
    #[actix::test]
    async fn a_leave_this_account_announced_drops_the_namespaces_topic() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = ContextGroupId::from([0x95; 32]);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::AccountNamespaceLeft {
            group_id: account_namespace.to_bytes(),
            namespace: left,
            purge: false,
        });

        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();
        assert!(
            seen.contains(&topic(left)),
            "a leave the account announced has to drop the namespace's topic"
        );
    }

    /// A leave the account asked every device to purge on takes the namespace
    /// and its contexts off this one too, and leaves no forgotten marker.
    #[actix::test]
    async fn a_purging_leave_this_account_announced_takes_the_namespace_off_this_node() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = ContextGroupId::from([0x96; 32]);
        let context = calimero_primitives::context::ContextId::from([0x9C; 32]);
        MetaRepository::new(&store)
            .save(
                &left,
                &calimero_governance_store::test_fixtures::test_meta(),
            )
            .expect("the namespace");
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&left)
            .expect("take part");
        calimero_governance_store::register_context_in_group(&store, &left, &context)
            .expect("register");
        let state = [[0x9C; 32].to_vec(), vec![1; 32]].concat();
        store
            .raw_put(calimero_store::db::Column::State, &state, b"value")
            .expect("write");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::AccountNamespaceLeft {
            group_id: account_namespace.to_bytes(),
            namespace: left,
            purge: true,
        });

        let purged =
            eventually(|| MetaRepository::new(&store).load(&left).unwrap().is_none()).await;
        listener.abort();
        assert!(purged, "the namespace is gone from this device");
        assert!(store
            .raw_get(calimero_store::db::Column::State, &state)
            .unwrap()
            .is_none());
        assert!(!NamespaceRepository::new(&store)
            .participating_namespaces()
            .unwrap()
            .contains(&left));
        assert_eq!(ForgottenNamespaces::new(&store).list().unwrap(), vec![]);
    }

    /// A seeded namespace this node takes part in, targeting nothing.
    fn a_held_namespace(store: &Store, seed: u8) -> ContextGroupId {
        let namespace = ContextGroupId::from([seed; 32]);
        MetaRepository::new(store)
            .save(
                &namespace,
                &calimero_governance_store::test_fixtures::test_meta(),
            )
            .expect("the namespace");
        let _identity = NamespaceRepository::new(store)
            .participate_in(&namespace)
            .expect("take part");
        namespace
    }

    fn this_nodes_account(store: &Store) -> AccountId {
        NodeDeviceRepository::new(store)
            .get()
            .expect("read the device")
            .expect("this node is a device")
            .account
    }

    fn purge_announced(account_namespace: ContextGroupId, namespace: ContextGroupId) {
        op_events::notify(OpEvent::AccountNamespaceLeft {
            group_id: account_namespace.to_bytes(),
            namespace,
            purge: true,
        });
    }

    /// The purge op is signed by any device that speaks for the account's admin, so
    /// a sibling purges only a namespace its account no longer belongs to. The
    /// sentinel purge, queued behind it, is applied only once this one was decided.
    #[actix::test]
    async fn a_purge_for_a_namespace_the_account_still_belongs_to_is_ignored() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let (kept, sentinel) = (
            a_held_namespace(&store, 0x97),
            a_held_namespace(&store, 0x98),
        );
        MembershipRepository::new(&store)
            .add_member(&kept, &this_nodes_account(&store), GroupMemberRole::Member)
            .expect("the account is a member");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        purge_announced(account_namespace, kept);
        purge_announced(account_namespace, sentinel);
        let sentinel_gone = eventually(|| {
            MetaRepository::new(&store)
                .load(&sentinel)
                .unwrap()
                .is_none()
        })
        .await;
        listener.abort();

        assert!(
            sentinel_gone,
            "the sentinel purge applied after the first was decided"
        );
        assert!(
            MetaRepository::new(&store).load(&kept).unwrap().is_some(),
            "the account is still a member, so the purge was ignored"
        );
    }

    /// The purge op can reach a sibling before the `MemberLeft` it follows. It
    /// waits for it instead of being dropped.
    #[actix::test]
    async fn a_purge_that_arrives_before_the_leave_waits_for_it() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = a_held_namespace(&store, 0x99);
        let account = this_nodes_account(&store);
        MembershipRepository::new(&store)
            .add_member(&left, &account, GroupMemberRole::Member)
            .expect("the account is a member");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let sentinel = a_held_namespace(&store, 0x9A);
        purge_announced(account_namespace, left);
        purge_announced(account_namespace, sentinel);
        assert!(
            eventually(|| MetaRepository::new(&store)
                .load(&sentinel)
                .unwrap()
                .is_none())
            .await,
            "the sentinel purge was applied behind the first"
        );
        assert!(
            MetaRepository::new(&store).load(&left).unwrap().is_some(),
            "the purge came first and had to wait"
        );
        // The leave folds afterwards: the member row goes, and its event fires.
        MembershipRepository::new(&store)
            .remove_member(&left, &account)
            .expect("the account left");
        op_events::notify(OpEvent::MemberRemoved {
            group_id: left.to_bytes(),
            member: account,
        });
        let purged =
            eventually(|| MetaRepository::new(&store).load(&left).unwrap().is_none()).await;
        listener.abort();

        assert!(
            purged,
            "the purge that came first applied once the leave did"
        );
    }

    /// The wait for the leave survives a restart: a node that comes back after the
    /// leave folded purges what it was waiting to purge.
    #[actix::test]
    async fn a_purge_waiting_for_the_leave_is_done_after_a_restart() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = a_held_namespace(&store, 0x9B);
        let account = this_nodes_account(&store);
        MembershipRepository::new(&store)
            .add_member(&left, &account, GroupMemberRole::Member)
            .expect("the account is a member");
        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        purge_announced(account_namespace, left);
        assert!(
            eventually(|| PendingNamespacePurges::new(&store).is_armed(&left).unwrap()).await,
            "the purge is waiting for the leave"
        );
        listener.abort();

        // The leave folds while the node is down: no event is delivered for it.
        MembershipRepository::new(&store)
            .remove_member(&left, &account)
            .expect("the account left");
        let listener = listen(&store, &harness);
        let purged =
            eventually(|| MetaRepository::new(&store).load(&left).unwrap().is_none()).await;
        listener.abort();

        assert!(purged, "the start-up pass finished the purge");
        assert!(!PendingNamespacePurges::new(&store).is_armed(&left).unwrap());
    }

    /// Any device of the account can arm a purge while the account is still a member,
    /// so a later plain leave (or a kick of someone else) must not turn into one.
    #[actix::test]
    async fn a_purge_armed_by_another_device_does_not_survive_a_plain_leave() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let (held, sentinel) = (
            a_held_namespace(&store, 0x9C),
            a_held_namespace(&store, 0x9E),
        );
        let account = this_nodes_account(&store);
        MembershipRepository::new(&store)
            .add_member(&held, &account, GroupMemberRole::Member)
            .expect("the account is a member");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        purge_announced(account_namespace, held);
        assert!(eventually(|| PendingNamespacePurges::new(&store).is_armed(&held).unwrap()).await);
        // The account then leaves plainly: the pending purge is withdrawn.
        op_events::notify(OpEvent::AccountNamespaceLeft {
            group_id: account_namespace.to_bytes(),
            namespace: held,
            purge: false,
        });
        assert!(
            eventually(|| !PendingNamespacePurges::new(&store).is_armed(&held).unwrap()).await,
            "a plain leave withdraws the pending purge"
        );
        // The account's own removal follows.
        MembershipRepository::new(&store)
            .remove_member(&held, &account)
            .expect("the account left");
        op_events::notify(OpEvent::MemberRemoved {
            group_id: held.to_bytes(),
            member: account,
        });
        PendingNamespacePurges::new(&store)
            .arm(&sentinel)
            .expect("arm the sentinel");
        op_events::notify(OpEvent::MemberRemoved {
            group_id: sentinel.to_bytes(),
            member: account,
        });
        let sentinel_gone = eventually(|| {
            MetaRepository::new(&store)
                .load(&sentinel)
                .unwrap()
                .is_none()
        })
        .await;
        listener.abort();

        assert!(sentinel_gone, "the events were handled");
        assert!(
            MetaRepository::new(&store).load(&held).unwrap().is_some(),
            "the account left plainly, so the namespace stays"
        );
    }

    /// A rejoin ends the wait: a removal and a re-add can fold in one batch, so the
    /// armed purge is withdrawn when the account gains the namespace again.
    #[actix::test]
    async fn a_purge_armed_before_a_rejoin_does_not_outlive_it() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let (held, sentinel) = (
            a_held_namespace(&store, 0x9F),
            a_held_namespace(&store, 0xA0),
        );
        let account = this_nodes_account(&store);
        MembershipRepository::new(&store)
            .add_member(&held, &account, GroupMemberRole::Member)
            .expect("the account is a member");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        purge_announced(account_namespace, held);
        assert!(eventually(|| PendingNamespacePurges::new(&store).is_armed(&held).unwrap()).await);
        op_events::notify(OpEvent::AccountNamespaceGained {
            group_id: account_namespace.to_bytes(),
            namespace: held,
            application: None,
        });
        assert!(
            eventually(|| !PendingNamespacePurges::new(&store).is_armed(&held).unwrap()).await,
            "gaining the namespace again withdraws the pending purge"
        );
        // A later removal of the account must not finish the withdrawn purge.
        PendingNamespacePurges::new(&store)
            .arm(&sentinel)
            .expect("arm the sentinel");
        MembershipRepository::new(&store)
            .remove_member(&held, &account)
            .expect("the account left again");
        op_events::notify(OpEvent::MemberRemoved {
            group_id: held.to_bytes(),
            member: account,
        });
        op_events::notify(OpEvent::MemberRemoved {
            group_id: sentinel.to_bytes(),
            member: account,
        });
        let sentinel_gone = eventually(|| {
            MetaRepository::new(&store)
                .load(&sentinel)
                .unwrap()
                .is_none()
        })
        .await;
        listener.abort();

        assert!(sentinel_gone, "the events were handled");
        assert!(MetaRepository::new(&store).load(&held).unwrap().is_some());
    }

    /// The purge op applies from any device that is an admin of the account
    /// namespace, one descoped from the namespace included: the apply checks the
    /// signer's standing, not its scope. What bounds it is here: a sibling ignores it
    /// while the account is a member.
    #[actix::test]
    async fn a_purge_from_a_device_descoped_from_the_namespace_does_nothing_while_the_account_is_a_member(
    ) {
        let store = store();
        // This node's device covers app 0x11 only; the namespace targets app 0x22.
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let (held, sentinel) = (
            a_held_namespace(&store, 0xA1),
            a_held_namespace(&store, 0xA2),
        );
        for namespace in [held, sentinel] {
            let mut meta = MetaRepository::new(&store)
                .load(&namespace)
                .unwrap()
                .expect("the namespace");
            meta.target.application_id = app(0x22);
            MetaRepository::new(&store).save(&namespace, &meta).unwrap();
        }
        MembershipRepository::new(&store)
            .add_member(&held, &this_nodes_account(&store), GroupMemberRole::Member)
            .expect("the account is a member");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        purge_announced(account_namespace, held);
        purge_announced(account_namespace, sentinel);
        let sentinel_gone = eventually(|| {
            MetaRepository::new(&store)
                .load(&sentinel)
                .unwrap()
                .is_none()
        })
        .await;
        listener.abort();

        assert!(sentinel_gone, "the events were handled");
        assert!(
            MetaRepository::new(&store).load(&held).unwrap().is_some(),
            "the account still belongs to it, so nothing was purged"
        );
    }

    /// Nothing re-drives the certified op, so the sweep is what lets the uncovered
    /// topic go - and nothing else, the account namespace least of all.
    #[actix::test]
    async fn the_start_up_sweep_drops_what_this_devices_scope_no_longer_covers() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let kept = ContextGroupId::from([0x97; 32]);
        let dropped = ContextGroupId::from([0x98; 32]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(kept, Some(app(0x11))).expect("still in scope");
        set.record(dropped, Some(app(0x22)))
            .expect("no longer in scope");

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(dropped))).await;
        listener.abort();

        assert!(
            seen.contains(&topic(dropped)),
            "the narrowed-away namespace has to be unfollowed; got: {seen:?}"
        );
        assert!(!seen.contains(&topic(kept)), "got: {seen:?}");
        assert!(!seen.contains(&topic(account_namespace)), "got: {seen:?}");
    }

    /// The arm itself, isolated from the sweep by recording the set row only
    /// after the listener has already swept an empty one.
    #[actix::test]
    async fn this_devices_own_narrowing_drops_the_namespaces_it_stopped_covering() {
        let store = store();
        let (account_namespace, device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let dropped = ContextGroupId::from([0x99; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(dropped, Some(app(0x22)))
            .expect("a namespace this device's scope does not reach");
        op_events::notify(OpEvent::AccountDeviceCertified {
            group_id: account_namespace.to_bytes(),
            device,
        });

        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(dropped))).await;
        listener.abort();

        assert!(
            seen.contains(&topic(dropped)),
            "this device's own scope arriving has to drop what it no longer covers; got: {seen:?}"
        );
    }

    /// A burst of this device's own certificates - a narrowed scope and the one
    /// that replaced it, folded together when a catch-up reaches the account
    /// namespace - must leave the topics matching the NEWER scope.
    ///
    /// Each event's task used to read the scope and act on it independently, so
    /// the task that read the narrowed scope could drop a namespace after the one
    /// that read the replacement had kept it, and no later event re-followed it.
    /// `account-pairing-application-scope` failed on exactly this: the device was
    /// paired at scope B, then A, before it followed its account namespace, and it
    /// ended up off namespace A1's topic for good. The narrowed task is held here
    /// on the subscribe it makes before its unsubscribe, which is the window that
    /// interleaving needs.
    #[actix::test]
    async fn a_superseded_scope_does_not_drop_a_topic_the_newer_scope_keeps() {
        let store = store();
        let (account_namespace, device, root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let kept = ContextGroupId::from([0xA7; 32]);
        let narrowed_to = ContextGroupId::from([0xA8; 32]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(kept, Some(app(0x11)))
            .expect("covered by the first scope and the last");

        let (mut harness, release) =
            actor::over_holding_subscribe(store.clone(), topic(narrowed_to)).await;
        let listener = listen(&store, &harness);
        assert!(
            eventually(|| harness.live_topics().contains(&topic(kept))).await,
            "the start-up sweep follows what the first scope covers"
        );

        set.record(narrowed_to, Some(app(0x22)))
            .expect("covered by the narrowed scope alone");
        rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app(0x22)],
            1,
        );
        op_events::notify(OpEvent::AccountDeviceCertified {
            group_id: account_namespace.to_bytes(),
            device,
        });
        let mut asked = Vec::new();
        assert!(
            eventually(|| {
                asked.append(&mut harness.subscribed());
                asked.contains(&topic(narrowed_to))
            })
            .await,
            "the narrowed scope's task has read it and is held part-way through acting on it"
        );

        rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app(0x11)],
            2,
        );
        op_events::notify(OpEvent::AccountDeviceCertified {
            group_id: account_namespace.to_bytes(),
            device,
        });
        // As far as the replacement's task can get before the held one moves on.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        release.add_permits(1);

        // Both tasks have finished once each has dropped what its scope left out:
        // the narrowed one `kept`, the replacement `narrowed_to`.
        let dropped = unsubscribes_until(&mut harness, |seen| {
            seen.contains(&topic(kept)) && seen.contains(&topic(narrowed_to))
        })
        .await;
        let live = harness.live_topics();
        listener.abort();
        assert!(
            dropped.contains(&topic(kept)) && dropped.contains(&topic(narrowed_to)),
            "both scopes have to be acted on; dropped: {dropped:?}"
        );
        assert_eq!(
            live,
            [topic(kept)].into_iter().collect(),
            "the topics have to match the newest scope, which keeps {kept:?} alone"
        );
    }

    /// The other half of the start-up sweep. Nothing re-drives a leave applied
    /// while no listener was up, and the topic would stay subscribed forever.
    #[actix::test]
    async fn a_namespace_left_while_nothing_listened_is_unfollowed_on_the_next_start() {
        let store = store();
        let (_account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = ContextGroupId::from([0x96; 32]);
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&left)
            .expect("this node still takes part in it");
        folded(&store, left);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);

        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();
        assert!(
            seen.contains(&topic(left)),
            "the sweep never unfollowed a namespace its account had already left"
        );
    }

    /// A namespace this node has not synced answers "absent" to both reads: the
    /// set names nothing, and a missing member row reads as not a member. Only
    /// a folded namespace can say the account actually left.
    #[actix::test]
    async fn an_unsynced_namespace_is_kept_because_nothing_says_the_account_left() {
        let store = store();
        let (_account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let namespaces = NamespaceRepository::new(&store);
        let unsynced = ContextGroupId::from([0x9A; 32]);
        let _unsynced = namespaces
            .participate_in(&unsynced)
            .expect("paired into it, and nothing about it has arrived yet");
        // A namespace that really was left, as the barrier: the sweep reaching
        // it is what says the sweep has run at all.
        let left = ContextGroupId::from([0x9B; 32]);
        let _left = namespaces.participate_in(&left).expect("takes part in it");
        folded(&store, left);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();

        assert!(seen.contains(&topic(left)), "the sweep has to have run");
        assert!(
            !seen.contains(&topic(unsynced)),
            "a namespace this node has never folded is unsynced, not left"
        );
    }

    /// The member row is the last word. This namespace is folded here and the
    /// set omits it, so only "the account is still a member of it" keeps it.
    #[actix::test]
    async fn a_namespace_the_set_omits_is_kept_while_the_account_is_still_a_member() {
        let store = store();
        let (account_namespace, _device, root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();
        let kept = ContextGroupId::from([0x97; 32]);
        let namespaces = NamespaceRepository::new(&store);
        let _identity = namespaces
            .participate_in(&kept)
            .expect("this node takes part in it");
        // Folded here, so the two conditions before the member row both pass and
        // the chain cannot short-circuit ahead of the one under test.
        folded(&store, kept);
        MembershipRepository::new(&store)
            .add_member(&kept, &account, GroupMemberRole::Member)
            .expect("and its account is still a member of it");

        // One barrier per half, so the assertion holds whichever runs first: the
        // follow half reaches `followed`, and the unfollow half drops `left`.
        let followed = ContextGroupId::from([0x98; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(followed, Some(app(0x11)))
            .expect("in scope");
        let left = ContextGroupId::from([0x99; 32]);
        let _identity = namespaces
            .participate_in(&left)
            .expect("takes part in it, and the set names it nowhere");
        folded(&store, left);

        let swept = |seen: &[String]| {
            seen.contains(&topic(left))
                && namespaces
                    .participating_namespaces()
                    .expect("read the participation rows")
                    .contains(&followed)
        };

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let seen = unsubscribes_until(&mut harness, &swept).await;
        listener.abort();

        assert!(swept(&seen), "both halves of the sweep have to have run");
        assert!(
            !seen.contains(&topic(kept)),
            "a namespace the account is still a member of must stay followed"
        );
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

    /// A binding for `device` in `namespace`, as a link op leaves one.
    fn a_device_bound_in(
        store: &Store,
        namespace: ContextGroupId,
        root_sk: &PrivateKey,
        device: DeviceId,
    ) {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            root_sk,
            genesis.account_id(),
            device,
            &PrivateKey::from([0x75; 32]).public_key(),
            &KemPublicKey::from([0x76; 32]),
            0,
            0,
        )
        .expect("the account root signs the cert the binding records");
        let recorded = AccountBindingRepository::new(store)
            .apply_link(&namespace, &genesis, &[], &cert, 0)
            .expect("write the binding");
        assert!(
            recorded.is_ok(),
            "the fixture's own link must be admissible"
        );
    }

    fn a_revocation_of(root_sk: &PrivateKey, device: DeviceId) -> SignedDeviceRevocation {
        let genesis = AccountGenesis::new(root_sk.public_key());
        SignedDeviceRevocation {
            genesis,
            chain: vec![],
            statement: DeviceRevocation::sign(root_sk, genesis.account_id(), device, 0)
                .expect("the account root signs the revocation"),
        }
    }

    /// The carry skips the account namespace, the topic it came from, and any
    /// namespace that already withdrew the device.
    #[test]
    fn a_proof_bearing_revocation_is_not_carried_where_already_withdrawn() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let still_live = ContextGroupId::from([0x84; 32]);
        let already_tombstoned = ContextGroupId::from([0x85; 32]);
        for namespace in [account_namespace, still_live, already_tombstoned] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part");
        }
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x66; 16],
        );
        for namespace in [account_namespace, still_live, already_tombstoned] {
            a_device_bound_in(&store, namespace, &root_sk, lost);
        }
        AccountBindingRepository::new(&store)
            .apply_revocation(&already_tombstoned, lost)
            .expect("tombstone one of them ahead of the carry");
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        assert_eq!(
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), account, lost),
            vec![still_live]
        );
        assert_eq!(
            namespaces_to_revoke_in(&store, OTHER_GROUP, account, lost),
            Vec::<ContextGroupId>::new(),
            "a revocation folded on a project's DAG is this node's own publish coming back"
        );
    }

    /// A namespace the device never linked in still gets the withdrawal, so a
    /// link replayed there later is refused.
    #[test]
    fn a_proof_bearing_revocation_is_carried_where_the_device_never_linked() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let linked = ContextGroupId::from([0x86; 32]);
        let never_linked = ContextGroupId::from([0x87; 32]);
        for namespace in [account_namespace, linked, never_linked] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part");
        }
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x67; 16],
        );
        for namespace in [account_namespace, linked] {
            a_device_bound_in(&store, namespace, &root_sk, lost);
        }
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        let mut targets =
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), account, lost);
        targets.sort_by_key(ContextGroupId::to_bytes);
        assert_eq!(targets, vec![linked, never_linked]);
    }

    /// A narrowing is not a withdrawal: a namespace the device was only scoped out of
    /// still gets the carry, or the old scope would let it link there again.
    #[test]
    fn a_proof_bearing_revocation_is_carried_where_the_device_was_only_narrowed() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let narrowed = ContextGroupId::from([0x8B; 32]);
        for namespace in [account_namespace, narrowed] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part");
        }
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x6B; 16],
        );
        a_device_bound_in(&store, narrowed, &root_sk, lost);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();
        assert!(AccountBindingRepository::new(&store)
            .narrow(&narrowed, account, lost, 1)
            .expect("narrow the device out"));

        assert_eq!(
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), account, lost),
            vec![narrowed]
        );
    }

    /// Only this account's own device is carried: neither another account's proof nor
    /// a device id another account holds selects any of this node's namespaces.
    #[test]
    fn another_accounts_revocation_or_device_is_not_carried() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x88; 32]);
        let stranger = PrivateKey::from([0x89; 32]);
        let foreign = DeviceId::mint(
            AccountGenesis::new(stranger.public_key()).account_id(),
            [0x68; 16],
        );
        for namespace in [account_namespace, project] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part");
            a_device_bound_in(&store, namespace, &stranger, foreign);
        }
        let stranger_account = AccountGenesis::new(stranger.public_key()).account_id();
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        assert_eq!(
            namespaces_to_revoke_in(
                &store,
                account_namespace.to_bytes(),
                stranger_account,
                foreign
            ),
            Vec::<ContextGroupId>::new(),
            "another account's proof selects nothing"
        );
        assert_eq!(
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), account, foreign),
            Vec::<ContextGroupId>::new(),
            "nor does this account's proof for a device id another account holds there"
        );
    }

    /// The projection rule. A sibling reaches the namespaces this node takes part
    /// in that its scope covers: not one out of scope, not the account namespace.
    #[test]
    fn a_certified_sibling_is_bound_where_its_scope_reaches_and_nowhere_else() {
        let store = store();
        let (account_namespace, own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let covered = ContextGroupId::from([0x81; 32]);
        let elsewhere = ContextGroupId::from([0x82; 32]);
        a_namespace_targeting(&store, covered, app(0x11));
        a_namespace_targeting(&store, elsewhere, app(0x22));
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x63, &[app(0x11)]);

        let (cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("the sibling has somewhere to go");

        assert_eq!(cert.device(), sibling);
        assert_eq!(targets, vec![covered]);
        assert!(
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), own_device).is_none(),
            "this node never publishes its own link; the holder that signed it did"
        );
        assert!(
            namespaces_to_bind_into(&store, OTHER_GROUP, sibling).is_none(),
            "the same op folded in a project namespace is this node's own publish coming back"
        );
    }

    /// An unscoped sibling reaches everything this node takes part in - except
    /// the account namespace, which an empty scope would otherwise sweep up.
    #[test]
    fn an_unscoped_sibling_still_skips_the_account_namespace() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x83; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x64, &[]);

        let (_cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("an unscoped sibling belongs everywhere this node is");

        assert_eq!(targets, vec![project]);
    }

    /// A revoked id is spent everywhere, so there is nowhere left to project it -
    /// the target's own tombstone is no gate, a stranger revoker never wrote one.
    #[test]
    fn a_sibling_revoked_anywhere_is_projected_nowhere() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x85; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x66, &[]);
        AccountBindingRepository::new(&store)
            .apply_revocation(&account_namespace, sibling)
            .expect("tombstone the sibling where this node did hear of it");

        assert!(
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling).is_none(),
            "a device revoked in one namespace this node takes part in must reach no other"
        );
    }

    /// This node as a member of `namespace`, under the identity participating
    /// minted: the apply resolves a link's endorser through its binding.
    fn a_member_of(store: &Store, namespace: ContextGroupId) {
        let (sign_pk, _secret) = NamespaceRepository::new(store)
            .resolve_identity(&namespace)
            .expect("read this node's identity here")
            .expect("taking part in a namespace mints one");
        let account = crate::test_support::enrol(store, &namespace, &sign_pk);
        MembershipRepository::new(store)
            .add_member(&namespace, &account, GroupMemberRole::Member)
            .expect("and a member of it");
    }

    /// The arm itself, driven the way the sweep tests drive it. Pinned on the
    /// binding, which the publish path applies locally before it broadcasts.
    #[actix::test]
    async fn a_sibling_certified_on_the_account_topic_is_published_into_a_namespace_here() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x86; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _key_id = GroupKeyring::new(&store, project)
            .store_key(&[0x42; 32])
            .expect("hold this namespace's scope key, without which nothing is published");
        a_member_of(&store, project);
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x67, &[app(0x11)]);

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::AccountDeviceCertified {
            group_id: account_namespace.to_bytes(),
            device: sibling,
        });

        let bindings = AccountBindingRepository::new(&store);
        let bound = eventually(|| {
            bindings
                .is_device_linked(&project, sibling)
                .expect("read the sibling's binding row")
        })
        .await;
        listener.abort();

        assert!(
            bound,
            "the listener never bound a certified sibling into the namespace this \
             node takes part in that its scope covers"
        );
    }

    /// The revoked-anywhere rule, re-read after the wait. A revocation that folds
    /// while the projection sleeps has to stop it publishing anything.
    #[actix::test]
    async fn a_sibling_revoked_during_the_wait_is_published_nowhere() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x8B; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _key_id = GroupKeyring::new(&store, project)
            .store_key(&[0x42; 32])
            .expect("hold this namespace's scope key, without which nothing is published");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x6B, &[app(0x11)]);
        let (cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("the decision is taken before the wait");
        let signer_sk =
            PrivateKey::from(signing_identity(&store, &targets).expect("an identity to sign with"));

        let mut harness = actor::over(store.clone()).await;
        let _started = harness.broadcast_topics();
        // Tombstoned where no target would see it, so only the re-read can refuse.
        AccountBindingRepository::new(&store)
            .apply_revocation(&account_namespace, sibling)
            .expect("the revocation folds while the projection waits");
        publish_sibling_link(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &targets,
            &signer_sk,
            &cert,
        )
        .await;

        assert!(
            !harness.broadcast_topics().contains(&topic(project)),
            "a sibling revoked while the projection waited must reach no namespace"
        );
    }

    /// The re-read before each publish. A sibling that got there first between
    /// the decision and this publish leaves nothing here to withdraw.
    #[actix::test]
    async fn a_namespace_a_sibling_already_withdrew_from_is_not_published_into() {
        let store = store();
        let (_account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let still_live = ContextGroupId::from([0x88; 32]);
        let withdrawn = ContextGroupId::from([0x89; 32]);
        for namespace in [still_live, withdrawn] {
            a_namespace_targeting(&store, namespace, app(0x11));
            let _key_id = GroupKeyring::new(&store, namespace)
                .store_key(&[0x42; 32])
                .expect("hold the scope key");
        }
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x69; 16],
        );
        for namespace in [still_live, withdrawn] {
            a_device_bound_in(&store, namespace, &root_sk, lost);
        }
        // What a faster sibling's carry left behind after this node decided.
        AccountBindingRepository::new(&store)
            .apply_revocation(&withdrawn, lost)
            .expect("a faster sibling withdrew it here");
        let account = AccountGenesis::new(root_sk.public_key()).account_id();
        let op = GroupOp::AccountDeviceUnlinked {
            account,
            device: lost,
            proof: Some(a_revocation_of(&root_sk, lost)),
        };
        let signer_sk = PrivateKey::from(
            signing_identity(&store, &[still_live, withdrawn]).expect("an identity to sign with"),
        );

        let mut harness = actor::over(store.clone()).await;
        let _started = harness.broadcast_topics();
        carry_into(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &[still_live, withdrawn],
            &signer_sk,
            account,
            lost,
            &op,
        )
        .await;

        let seen = harness.broadcast_topics();
        assert!(
            seen.contains(&topic(still_live)),
            "the namespace that still has the device linked must get the withdrawal"
        );
        assert!(
            !seen.contains(&topic(withdrawn)),
            "a namespace a sibling already withdrew from must get nothing at all"
        );
    }

    /// The arm itself, driven the way the sweep tests drive it: the listener
    /// folds a proof-bearing revocation and the tombstone lands where it must.
    #[actix::test]
    async fn a_revocation_on_the_account_topic_is_carried_into_a_namespace_here() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let elsewhere = ContextGroupId::from([0x8A; 32]);
        a_namespace_targeting(&store, elsewhere, app(0x11));
        let _key_id = GroupKeyring::new(&store, elsewhere)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x6A; 16],
        );
        a_device_bound_in(&store, account_namespace, &root_sk, lost);
        a_device_bound_in(&store, elsewhere, &root_sk, lost);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::DeviceRevoked {
            group_id: account_namespace.to_bytes(),
            account,
            device: lost,
            proof: Some(Box::new(a_revocation_of(&root_sk, lost))),
        });

        let bindings = AccountBindingRepository::new(&store);
        let carried = eventually(|| {
            bindings
                .is_revoked(&elsewhere, lost)
                .expect("read the device's tombstone row")
        })
        .await;
        listener.abort();
        assert!(
            carried,
            "the listener never carried a proof-bearing revocation off the account topic"
        );
        assert!(
            !bindings
                .is_revoked(&account_namespace, lost)
                .expect("read the account namespace's tombstone row"),
            "the account namespace is where the revocation came FROM; republishing there echoes"
        );
    }

    /// A namespace whose replay was missed gets this account's recorded withdrawals
    /// when the listener starts, so a dropped replay is repaired.
    #[actix::test]
    async fn the_start_up_sweep_replays_this_accounts_withdrawals() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let missed = ContextGroupId::from([0x8C; 32]);
        a_namespace_targeting(&store, missed, app(0x11));
        let _key_id = GroupKeyring::new(&store, missed)
            .store_key(&[0x42; 32])
            .expect("hold the scope key the replay publishes under");
        let lost = DeviceId::mint(
            AccountGenesis::new(root_sk.public_key()).account_id(),
            [0x6C; 16],
        );
        AccountDeviceRegistry::new(&store, account_namespace)
            .record_revocation(&a_revocation_of(&root_sk, lost))
            .expect("the account namespace holds the withdrawal");
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let bindings = AccountBindingRepository::new(&store);
        let replayed = eventually(|| {
            bindings
                .device_is_withdrawn(&missed, account, lost)
                .expect("read the withdrawal")
        })
        .await;
        listener.abort();

        assert!(
            replayed,
            "the start-up sweep never replayed the recorded withdrawal"
        );
        assert!(
            harness.broadcast_topics().contains(&topic(missed)),
            "the replayed withdrawal must be published, not only applied here"
        );
    }
}
